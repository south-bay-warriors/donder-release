use anyhow::{Result, Ok, bail};
use semver::Version;
use std::process::Command;
use regex::Regex;

#[derive(Debug, Default)]
pub struct Git {
    repo_url: String,
    token: String,
    author: String,
    email: String,
    pub owner: String,
    pub repo: String,
    /// Original env var values to restore on drop
    original_env: Vec<(String, Option<String>)>,
}

/// Resolves value from environment variables with cross-fallback.
/// Tries `primary` env var first, then `fallback`, then returns the default.
fn resolve_env(primary: &str, fallback: &str, default: &str) -> String {
    std::env::var(primary)
        .or_else(|_| std::env::var(fallback))
        .unwrap_or(default.to_string())
}

const DEFAULT_NAME: &str = "sbayw-bot";
const DEFAULT_EMAIL: &str = "support@southbaywarriors.com";

impl Git {
    pub fn new(token: &str) -> Result<Self> {
        // Save original env var state before making any changes.
        // Original values are restored when Git is dropped.
        let env_keys = ["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_COMMITTER_NAME", "GIT_COMMITTER_EMAIL"];
        let original_env: Vec<(String, Option<String>)> = env_keys
            .iter()
            .map(|key| (key.to_string(), std::env::var(key).ok()))
            .collect();

        // Resolve git identity from environment variables with cross-fallback
        let author = resolve_env("GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME", DEFAULT_NAME);
        let email = resolve_env("GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL", DEFAULT_EMAIL);

        // Set only missing identity env vars so git picks them up.
        // This ensures git commit works in environments without a global git config (e.g. CI runners).
        // Safety: called during single-threaded initialization before tokio runtime starts.
        let env_values = [&author, &email, &author, &email];
        unsafe {
            for (key, value) in env_keys.iter().zip(env_values.iter()) {
                if std::env::var(key).is_err() {
                    std::env::set_var(key, value);
                }
            }
        }

        let origin_url = Command::new("git")
            .arg("config")
            .arg("--get")
            .arg("remote.origin.url")
            .output()
            .expect("[get_origin_url] failed to get origin url");

        let origin_url = String::from_utf8_lossy(&origin_url.stdout).trim().to_string();

        // get host, owner and repo from git remote url with regex
        let re = Regex::new(r"(git@|https://)([\w\.@]+)(/|:)([\w,\-,_]+)/([\w,\-,_]+)(.git){0,1}((/){0,1})").unwrap();
        let caps = re.captures(&origin_url).unwrap();

        Ok(
            Self {
                repo_url: format!("https://{}@{}/{}/{}.git", token, &caps[2], &caps[4], &caps[5]),
                token: token.to_string(),
                author,
                email,
                owner: caps[4].to_string(),
                repo: caps[5].to_string(),
                original_env,
            }
        )
    }

    pub fn sync(&self) -> Result<()> {
        let output = Command::new("git")
            .arg("status")
            .output()
            .expect("[sync] failed to fetch all");

        let output = String::from_utf8_lossy(&output.stdout);

        if !output.contains("nothing to commit, working tree clean") {
           bail!("There are uncommitted changes. Please commit or stash them before running donder-release.");
        }

        // removed because it was causing git merge conflicts - the user must make sure the local branch is up to date
        // pull changes from remote
        // Command::new("git")
        //     .args(["pull", &self.repo_url])
        //     .output()?;

        // fetch tags from remote
        Command::new("git")
            .args(["fetch", "--prune", "--prune-tags", &self.repo_url])
            .output()?;

        Ok(())
    }

    pub fn origin_url(&self) -> Result<String> {
        let url = self.repo_url
            .replace(&format!("{}@", self.token), "")
            .replace(".git", "");
        Ok(url)
    }

    pub fn get_tags(&self, prefix: &str) -> Result<Vec<ReleaseInfo>> {
        let output = Command::new("git")
            .args(["tag", "-l"])
            .output()?;

        if !output.status.success() {
            bail!("failed to get tags");
        }
            
        let output = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let mut tags = output.split_whitespace().collect::<Vec<&str>>();

        tags.retain(
                |tag| tag.starts_with(prefix) && Version::parse(&tag.replace(prefix, "")).is_ok()
            );

        // map tags to tag info
        let mut tags_info = tags
            .iter()
            .map(|tag| ReleaseInfo::new(tag, prefix, false))
            .collect::<Vec<ReleaseInfo>>();

        // sort tags by version
        tags_info.sort_by(|a, b| b.version.cmp(&a.version));

        Ok(tags_info)
    }

    pub fn tag_head(&self, tag: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-list", "-1", tag])
            .output()?;

        if !output.status.success() {
            bail!("failed to get tag head");
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn get_commits(&self, tag_head: &str, package_path: &str) -> Result<Vec<Commit>> {
        // get commits between tag_head and HEAD
        let output = match tag_head.is_empty() {
            true => match package_path.is_empty() {
                true => Command::new("git")
                    .args(["log", "--pretty=format:\"%h|||%s|||%b\""])
                    .output()
                    .expect("[get_commits] failed to fetch"),
                false => Command::new("git")
                    .args(["log", "--pretty=format:\"%h|||%s|||%b\"", package_path])
                    .output()
                    .expect("[get_commits] failed to fetch"),
            },
            false => match package_path.is_empty() {
                true => Command::new("git")
                    .args(["log", "--pretty=format:\"%h|||%s|||%b\"", &format!("{}..HEAD", tag_head)])
                    .output()
                    .expect("[get_commits] failed to fetch"),
                false => Command::new("git")
                    .args(["log", "--pretty=format:\"%h|||%s|||%b\"", &format!("{}..HEAD", tag_head), "--", package_path])
                    .output()
                    .expect("[get_commits] failed to fetch"),
            }
        };

        let output = String::from_utf8_lossy(&output.stdout).to_string();

        let commits = output
            .split("\n")
            .map(|commit| {
                let commit = commit.trim_matches(|c| c == '\"').split("|||").collect::<Vec<&str>>();
                match commit.len() {
                    3 => Commit::new(commit[0], commit[1], commit[2]),
                    2 => Commit::new(commit[0], "", ""),
                    _ => Commit::new("", "", ""),
                }
            })
            .collect::<Vec<Commit>>();

        Ok(commits)
    }

    pub fn tag(&self, tag: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["tag", "-a", tag, "-m", tag])
            .output()?;

        if !output.status.success() {
            bail!("failed to tag");
        }

        Ok(())
    }

    pub fn commit(&self, message: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["add", "--all",])
            .output()?;

        if !output.status.success() {
            bail!("failed to add changes");
        }

        let output = Command::new("git")
            .args(["commit", &format!("--author=\"{} <{}>\"", self.author, self.email), "-m", message])
            .output()?;

        if !output.status.success() {
            bail!(format!("failed to commit changes: {}", String::from_utf8_lossy(&output.stderr)));
        }

        Ok(())
    }

    // push commit
    pub fn push(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["push", &format!("--repo={}", &self.repo_url.as_str())])
            .output()?;

        // check if push was successful
        if !output.status.success() {
            self.undo_commit()?;
            bail!("failed to push changes token may be invalid: {}", String::from_utf8_lossy(&output.stderr));
        }

        Ok(())
    }

    // push tag
    pub fn push_tag(&self, tag: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["push", &self.repo_url.as_str(), tag])
            .output()?;

        // check if push was successful
        if !output.status.success() {
            self.undo_tag(tag)?;
            bail!(format!("failed to push tag: {}", String::from_utf8_lossy(&output.stderr)));
        }

        Ok(())
    }

    // delete tag on remote
    pub fn delete_tag(&self, tag: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["push", "--delete", &self.repo_url.as_str(), tag])
            .output()?;

        // check if push was successful
        if !output.status.success() {
            bail!(format!("failed to delete tag on remote: {}", String::from_utf8_lossy(&output.stderr)));
        }

        Ok(())
    }

    // undo last tag
    pub fn undo_tag(&self, tag: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["tag", "-d", tag])
            .output()?;

        if !output.status.success() {
            bail!("failed to delete tag");
        }

        Ok(())
    }

    // undo last commit and changes
    pub fn undo_commit(&self) -> Result<()> {
        let output = Command::new("git")
            .args(["reset", "--hard", "HEAD^"])
            .output()?;

        if !output.status.success() {
            bail!("failed to undo commit");
        }

        Ok(())
    }
}

impl Drop for Git {
    fn drop(&mut self) {
        // Restore original env var values
        // Safety: called during shutdown, single-threaded context.
        unsafe {
            for (key, original) in &self.original_env {
                match original {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct ReleaseInfo {
    pub version: Version,
    pub prefix: String,
    pub head: String,
    pub initial: bool,
}

impl ReleaseInfo {
    pub fn new(tag: &str, prefix: &str, initial: bool) -> Self {
        Self {
            version: Version::parse(&tag.replace(&prefix, "")).unwrap(),
            prefix: prefix.to_string(),
            head: "".to_string(),
            initial,
        }
    }

    pub fn tag(&self) -> String {
        format!("{}{}", self.prefix, self.version)
    }

    pub fn update_head(&mut self, head: &str) {
        self.head = head.to_string();
    }
}

pub type Commits = Vec<Commit>;

#[derive(Debug)]
pub struct Commit {
    pub subject: String,
    pub body: String,
    pub hash: String,
}

impl Commit {
    pub fn new(hash: &str, subject: &str, body: &str) -> Self {
        Self {
            subject: subject.to_string(),
            body: body.to_string(),
            hash: hash.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_info_new_parses_tag() {
        let info = ReleaseInfo::new("v1.2.3", "v", false);
        assert_eq!(info.version, Version::parse("1.2.3").unwrap());
        assert_eq!(info.prefix, "v");
        assert!(!info.initial);
        assert!(info.head.is_empty());
    }

    #[test]
    fn release_info_new_with_prerelease() {
        let info = ReleaseInfo::new("v2.0.0-alpha.1", "v", false);
        assert_eq!(info.version, Version::parse("2.0.0-alpha.1").unwrap());
    }

    #[test]
    fn release_info_tag_formats_correctly() {
        let info = ReleaseInfo::new("v1.0.0", "v", false);
        assert_eq!(info.tag(), "v1.0.0");
    }

    #[test]
    fn release_info_tag_with_package_prefix() {
        let info = ReleaseInfo::new("my-pkg@v3.1.0", "my-pkg@v", false);
        assert_eq!(info.tag(), "my-pkg@v3.1.0");
    }

    #[test]
    fn release_info_update_head() {
        let mut info = ReleaseInfo::new("v1.0.0", "v", false);
        assert!(info.head.is_empty());
        info.update_head("abc123");
        assert_eq!(info.head, "abc123");
    }

    #[test]
    fn release_info_initial_flag() {
        let info = ReleaseInfo::new("v1.0.0", "v", true);
        assert!(info.initial);
    }

    #[test]
    fn commit_new_sets_fields() {
        let c = Commit::new("abc123", "feat: add feature", "some body");
        assert_eq!(c.hash, "abc123");
        assert_eq!(c.subject, "feat: add feature");
        assert_eq!(c.body, "some body");
    }

    #[test]
    fn commit_new_empty_fields() {
        let c = Commit::new("", "", "");
        assert!(c.hash.is_empty());
        assert!(c.subject.is_empty());
        assert!(c.body.is_empty());
    }

    // resolve_env tests

    #[test]
    fn resolve_env_returns_primary() {
        let primary = "GIT_AUTHOR_NAME";
        let fallback = "GIT_COMMITTER_NAME";
        let orig_primary = std::env::var(primary).ok();
        let orig_fallback = std::env::var(fallback).ok();

        unsafe { std::env::set_var(primary, "primary_val"); }
        unsafe { std::env::set_var(fallback, "fallback_val"); }

        let result = resolve_env(primary, fallback, "default");
        assert_eq!(result, "primary_val");

        unsafe {
            match &orig_primary { Some(v) => std::env::set_var(primary, v), None => std::env::remove_var(primary) }
            match &orig_fallback { Some(v) => std::env::set_var(fallback, v), None => std::env::remove_var(fallback) }
        }
    }

    #[test]
    fn resolve_env_falls_back_to_secondary() {
        let primary = "GIT_AUTHOR_EMAIL";
        let fallback = "GIT_COMMITTER_EMAIL";
        let orig_primary = std::env::var(primary).ok();
        let orig_fallback = std::env::var(fallback).ok();

        unsafe { std::env::remove_var(primary); }
        unsafe { std::env::set_var(fallback, "fallback_val"); }

        let result = resolve_env(primary, fallback, "default");
        assert_eq!(result, "fallback_val");

        unsafe {
            match &orig_primary { Some(v) => std::env::set_var(primary, v), None => std::env::remove_var(primary) }
            match &orig_fallback { Some(v) => std::env::set_var(fallback, v), None => std::env::remove_var(fallback) }
        }
    }

    #[test]
    fn resolve_env_returns_default() {
        let primary = "GIT_COMMITTER_NAME";
        let fallback = "GIT_AUTHOR_NAME";
        let orig_primary = std::env::var(primary).ok();
        let orig_fallback = std::env::var(fallback).ok();

        unsafe { std::env::remove_var(primary); }
        unsafe { std::env::remove_var(fallback); }

        let result = resolve_env(primary, fallback, "default_val");
        assert_eq!(result, "default_val");

        unsafe {
            match &orig_primary { Some(v) => std::env::set_var(primary, v), None => std::env::remove_var(primary) }
            match &orig_fallback { Some(v) => std::env::set_var(fallback, v), None => std::env::remove_var(fallback) }
        }
    }

    // env var save/restore tests

    fn make_git_with_env(original_env: Vec<(String, Option<String>)>) -> Git {
        Git {
            repo_url: String::new(),
            token: String::new(),
            author: String::new(),
            email: String::new(),
            owner: String::new(),
            repo: String::new(),
            original_env,
        }
    }

    #[test]
    fn drop_restores_git_author_name() {
        let key = "GIT_AUTHOR_NAME";
        let original = std::env::var(key).ok();

        unsafe { std::env::set_var(key, "my-name"); }
        let git = make_git_with_env(vec![(key.to_string(), Some("my-name".to_string()))]);

        unsafe { std::env::set_var(key, "sbayw-bot"); }
        drop(git);

        assert_eq!(std::env::var(key).unwrap(), "my-name");

        // Restore real original
        unsafe {
            match &original {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn drop_restores_git_author_email() {
        let key = "GIT_AUTHOR_EMAIL";
        let original = std::env::var(key).ok();

        unsafe { std::env::set_var(key, "me@example.com"); }
        let git = make_git_with_env(vec![(key.to_string(), Some("me@example.com".to_string()))]);

        unsafe { std::env::set_var(key, "bot@example.com"); }
        drop(git);

        assert_eq!(std::env::var(key).unwrap(), "me@example.com");

        unsafe {
            match &original {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn drop_restores_git_committer_name() {
        let key = "GIT_COMMITTER_NAME";
        let original = std::env::var(key).ok();

        unsafe { std::env::set_var(key, "my-name"); }
        let git = make_git_with_env(vec![(key.to_string(), Some("my-name".to_string()))]);

        unsafe { std::env::set_var(key, "sbayw-bot"); }
        drop(git);

        assert_eq!(std::env::var(key).unwrap(), "my-name");

        unsafe {
            match &original {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn drop_restores_git_committer_email() {
        let key = "GIT_COMMITTER_EMAIL";
        let original = std::env::var(key).ok();

        unsafe { std::env::set_var(key, "me@example.com"); }
        let git = make_git_with_env(vec![(key.to_string(), Some("me@example.com".to_string()))]);

        unsafe { std::env::set_var(key, "bot@example.com"); }
        drop(git);

        assert_eq!(std::env::var(key).unwrap(), "me@example.com");

        unsafe {
            match &original {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    #[test]
    fn drop_removes_git_committer_name_if_was_undefined() {
        let key = "GIT_COMMITTER_NAME";
        let original = std::env::var(key).ok();

        unsafe { std::env::remove_var(key); }
        let git = make_git_with_env(vec![(key.to_string(), None)]);

        unsafe { std::env::set_var(key, "sbayw-bot"); }
        drop(git);

        assert!(std::env::var(key).is_err());

        // Restore real original
        unsafe {
            if let Some(v) = &original {
                std::env::set_var(key, v);
            }
        }
    }

    #[test]
    fn drop_removes_git_committer_email_if_was_undefined() {
        let key = "GIT_COMMITTER_EMAIL";
        let original = std::env::var(key).ok();

        unsafe { std::env::remove_var(key); }
        let git = make_git_with_env(vec![(key.to_string(), None)]);

        unsafe { std::env::set_var(key, "bot@example.com"); }
        drop(git);

        assert!(std::env::var(key).is_err());

        unsafe {
            if let Some(v) = &original {
                std::env::set_var(key, v);
            }
        }
    }
}
