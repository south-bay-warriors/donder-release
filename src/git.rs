use anyhow::{Context, Result, Ok, bail};
use semver::{Version, Prerelease};
use std::process::Command;
use regex::Regex;
use chrono::Local;

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

/// Parses a git remote URL and returns (host, owner, repo).
fn parse_origin_url(url: &str) -> Result<(String, String, String)> {
    let re = Regex::new(r"(git@|https://)([\w\.@]+)(/|:)([\w,\-,_\.]+)/([\w,\-,_\.]+?)(?:\.git)?/?$").unwrap();
    let caps = re.captures(url)
        .context(format!("failed to parse remote url: {}", url))?;

    Ok((caps[2].to_string(), caps[4].to_string(), caps[5].to_string()))
}

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

        let (host, owner, repo) = parse_origin_url(&origin_url)?;

        Ok(
            Self {
                repo_url: format!("https://{}@{}/{}/{}.git", token, &host, &owner, &repo),
                token: token.to_string(),
                author,
                email,
                owner,
                repo,
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

        tags.retain(|tag| {
            if !tag.starts_with(prefix) {
                return false;
            }
            // Accept semver or CalVer (digits, dots, and optional pre-release)
            parse_version(&tag.replace(prefix, "")).is_some()
        });

        // map tags to tag info
        let mut tags_info = tags
            .iter()
            .map(|tag| ReleaseInfo::new(tag, prefix, false))
            .collect::<Vec<ReleaseInfo>>();

        sort_releases_desc(&mut tags_info);

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
        // Use %x00 (null byte) as commit separator to handle multi-line bodies
        let format = "%h|||%s|||%b%x00";

        // get commits between tag_head and HEAD
        let output = match tag_head.is_empty() {
            true => match package_path.is_empty() {
                true => Command::new("git")
                    .args(["log", &format!("--pretty=format:{}", format)])
                    .output()
                    .expect("[get_commits] failed to fetch"),
                false => Command::new("git")
                    .args(["log", &format!("--pretty=format:{}", format), package_path])
                    .output()
                    .expect("[get_commits] failed to fetch"),
            },
            false => match package_path.is_empty() {
                true => Command::new("git")
                    .args(["log", &format!("--pretty=format:{}", format), &format!("{}..HEAD", tag_head)])
                    .output()
                    .expect("[get_commits] failed to fetch"),
                false => Command::new("git")
                    .args(["log", &format!("--pretty=format:{}", format), &format!("{}..HEAD", tag_head), "--", package_path])
                    .output()
                    .expect("[get_commits] failed to fetch"),
            }
        };

        let output = String::from_utf8_lossy(&output.stdout).to_string();

        let commits = output
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(|commit| {
                let commit = commit.trim().split("|||").collect::<Vec<&str>>();
                match commit.len() {
                    3 => Commit::new(commit[0], commit[1], commit[2].trim()),
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

    // push release commit and tag together so the remote gets both or neither
    pub fn push_release(&self, tag: &str) -> Result<()> {
        let output = Command::new("git")
            .args(["push", "--atomic", &self.repo_url.as_str(), "HEAD", tag])
            .output()?;

        if output.status.success() {
            return Ok(());
        }

        let push_error = String::from_utf8_lossy(&output.stderr).trim().to_string();

        // A failed push doesn't prove the remote rejected it (the connection can drop after the
        // server applied the update), so check the remote before rolling anything back
        let remote_tag = match self.remote_tag_object(tag) {
            Result::Ok(remote_tag) => remote_tag,
            Err(e) => bail!(
                "failed to push release and could not check whether the remote has tag {} ({}), kept the local release commit and tag: {}",
                tag, e, push_error,
            ),
        };

        if remote_tag.is_some() && remote_tag == self.local_tag_object(tag).ok() {
            logInfo!("Push reported an error but the remote already has tag {}, continuing", tag);
            return Ok(());
        }

        // Undo both even if one fails
        let undo_tag = self.undo_tag(tag);
        let undo_commit = self.undo_commit();
        undo_tag.and(undo_commit)
            .with_context(|| format!("failed to push release ({}) and to roll back the local release commit and tag", push_error))?;

        bail!("failed to push release, token may be invalid: {}", push_error);
    }

    /// Object id of the tag on the remote, or None if the remote doesn't have it
    fn remote_tag_object(&self, tag: &str) -> Result<Option<String>> {
        let ref_name = format!("refs/tags/{}", tag);
        let output = Command::new("git")
            .args(["ls-remote", "--tags", &self.repo_url.as_str(), &ref_name])
            .output()?;

        if !output.status.success() {
            bail!("failed to list remote tags");
        }

        Ok(parse_ls_remote_ref(&String::from_utf8_lossy(&output.stdout), &ref_name))
    }

    fn local_tag_object(&self, tag: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["rev-parse", "--verify", &format!("refs/tags/{}", tag)])
            .output()?;

        if !output.status.success() {
            bail!("failed to resolve local tag");
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn tag_exists(&self, tag: &str) -> Result<bool> {
        let output = Command::new("git")
            .args(["rev-parse", "--quiet", "--verify", &format!("refs/tags/{}", tag)])
            .output()?;

        Ok(output.status.success())
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

/// Parses a semver or CalVer version string into a comparable `Version`.
/// CalVer segments may be zero-padded (e.g. `2026.09.3`), which strict semver rejects,
/// so the core segments are parsed as integers and the pre-release is kept as-is.
pub fn parse_version(version_str: &str) -> Option<Version> {
    if let std::result::Result::Ok(version) = Version::parse(version_str) {
        return Some(version);
    }

    let version_str = version_str.split('+').next().unwrap_or_default();
    let (core, pre) = match version_str.split_once('-') {
        Some((core, pre)) => (core, pre),
        None => (version_str, ""),
    };

    let parts = core
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect::<Option<Vec<u64>>>()?;

    if parts.len() != 3 {
        return None;
    }

    let mut version = Version::new(parts[0], parts[1], parts[2]);
    if !pre.is_empty() {
        version.pre = Prerelease::new(pre).ok()?;
    }

    Some(version)
}

/// Finds the object id for `ref_name` in `git ls-remote` output (`<id>\t<ref>` per line)
fn parse_ls_remote_ref(output: &str, ref_name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (id, name) = line.split_once('\t')?;
        (name == ref_name).then(|| id.to_string())
    })
}

/// Sorts releases newest first, comparing versions numerically so v1.2.10 > v1.2.9
fn sort_releases_desc(releases: &mut [ReleaseInfo]) {
    releases.sort_by(|a, b| b.version.cmp(&a.version));
}

#[derive(Debug)]
pub struct ReleaseInfo {
    pub version: Version,
    pub version_str: String,
    pub prefix: String,
    pub head: String,
    pub initial: bool,
}

impl ReleaseInfo {
    pub fn new(tag: &str, prefix: &str, initial: bool) -> Self {
        let version_str = tag.replace(prefix, "");
        let version = parse_version(&version_str).unwrap_or(Version::new(0, 0, 0));
        Self {
            version,
            version_str,
            prefix: prefix.to_string(),
            head: "".to_string(),
            initial,
        }
    }

    pub fn tag(&self) -> String {
        format!("{}{}", self.prefix, self.version_str)
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
    fn release_info_new_parses_zero_padded_calver() {
        let info = ReleaseInfo::new("v2026.09.3", "v", false);
        assert_eq!(info.version, Version::new(2026, 9, 3));
        assert_eq!(info.tag(), "v2026.09.3");
    }

    #[test]
    fn release_info_new_parses_zero_padded_calver_prerelease() {
        let info = ReleaseInfo::new("v26.09.0-beta.2", "v", false);
        assert_eq!(info.version, Version::parse("26.9.0-beta.2").unwrap());
    }

    // parse_ls_remote_ref tests

    #[test]
    fn parse_ls_remote_ref_finds_exact_ref() {
        let output = "8c2df03\trefs/tags/v1.0.2\n7ab897f\trefs/tags/v1.0.2^{}\n";
        assert_eq!(parse_ls_remote_ref(output, "refs/tags/v1.0.2"), Some("8c2df03".to_string()));
    }

    #[test]
    fn parse_ls_remote_ref_ignores_refs_sharing_a_suffix() {
        // ls-remote patterns match on the ref's tail, so a package tag can come back too
        let output = "aaaaaaa\trefs/tags/pkg@v1.0.2\n";
        assert_eq!(parse_ls_remote_ref(output, "refs/tags/v1.0.2"), None);
    }

    #[test]
    fn parse_ls_remote_ref_empty_output() {
        assert_eq!(parse_ls_remote_ref("", "refs/tags/v1.0.2"), None);
    }

    // parse_version tests

    #[test]
    fn parse_version_rejects_non_numeric_segments() {
        assert!(parse_version("1.2.x").is_none());
    }

    #[test]
    fn parse_version_rejects_wrong_segment_count() {
        assert!(parse_version("2026.09").is_none());
        assert!(parse_version("1.2.3.4").is_none());
    }

    // sort_releases_desc tests

    fn sorted_tags(tags: &[&str]) -> Vec<String> {
        let mut releases = tags
            .iter()
            .map(|tag| ReleaseInfo::new(tag, "v", false))
            .collect::<Vec<ReleaseInfo>>();
        sort_releases_desc(&mut releases);
        releases.iter().map(|r| r.tag()).collect()
    }

    #[test]
    fn sort_releases_two_digit_patch_is_newest() {
        assert_eq!(
            sorted_tags(&["v1.2.8", "v1.2.10", "v1.2.9"]),
            vec!["v1.2.10", "v1.2.9", "v1.2.8"],
        );
    }

    #[test]
    fn sort_releases_two_digit_minor_and_major() {
        assert_eq!(
            sorted_tags(&["v9.0.0", "v10.0.0", "v1.9.0", "v1.10.0"]),
            vec!["v10.0.0", "v9.0.0", "v1.10.0", "v1.9.0"],
        );
    }

    #[test]
    fn sort_releases_prerelease_below_release() {
        assert_eq!(
            sorted_tags(&["v1.2.10-beta.10", "v1.2.10", "v1.2.10-beta.9"]),
            vec!["v1.2.10", "v1.2.10-beta.10", "v1.2.10-beta.9"],
        );
    }

    #[test]
    fn sort_releases_calver_two_digit_micro_and_month() {
        assert_eq!(
            sorted_tags(&["v2026.09.9", "v2026.10.0", "v2026.09.10"]),
            vec!["v2026.10.0", "v2026.09.10", "v2026.09.9"],
        );
        assert_eq!(
            sorted_tags(&["v2026.9.3", "v2026.10.0"]),
            vec!["v2026.10.0", "v2026.9.3"],
        );
    }

    #[test]
    fn sort_releases_patch_digit_count_boundaries() {
        assert_eq!(
            sorted_tags(&["v1.2.99", "v1.2.1000", "v1.2.9", "v1.2.100", "v1.2.999", "v1.2.10"]),
            vec!["v1.2.1000", "v1.2.999", "v1.2.100", "v1.2.99", "v1.2.10", "v1.2.9"],
        );
    }

    #[test]
    fn sort_releases_minor_digit_count_boundaries() {
        assert_eq!(
            sorted_tags(&["v1.99.0", "v1.1000.0", "v1.9.0", "v1.100.0", "v1.999.0", "v1.10.0"]),
            vec!["v1.1000.0", "v1.999.0", "v1.100.0", "v1.99.0", "v1.10.0", "v1.9.0"],
        );
    }

    #[test]
    fn sort_releases_major_digit_count_boundaries() {
        assert_eq!(
            sorted_tags(&["v99.0.0", "v1000.0.0", "v9.0.0", "v100.0.0", "v999.0.0", "v10.0.0"]),
            vec!["v1000.0.0", "v999.0.0", "v100.0.0", "v99.0.0", "v10.0.0", "v9.0.0"],
        );
    }

    #[test]
    fn sort_releases_higher_segment_wins_over_more_digits_below() {
        // A larger lower segment must never outrank a larger higher segment
        assert_eq!(
            sorted_tags(&["v1.2.12345", "v1.3.0", "v1.99999.99999", "v2.0.0", "v10.0.0", "v9.99999.99999"]),
            vec!["v10.0.0", "v9.99999.99999", "v2.0.0", "v1.99999.99999", "v1.3.0", "v1.2.12345"],
        );
    }

    #[test]
    fn sort_releases_mixed_digit_counts_in_every_segment() {
        assert_eq!(
            sorted_tags(&["v12.345.6789", "v9.99999.0", "v12.345.678", "v12.99.99999", "v12.34.56789", "v123.4.5", "v12.3456.7", "v1.23456.789"]),
            vec!["v123.4.5", "v12.3456.7", "v12.345.6789", "v12.345.678", "v12.99.99999", "v12.34.56789", "v9.99999.0", "v1.23456.789"],
        );
    }

    #[test]
    fn sort_releases_is_independent_of_input_order() {
        let expected = vec!["v100.0.0", "v10.10.10", "v10.10.9", "v10.9.10", "v9.10.10", "v1.0.0"];
        let mut tags = expected.clone();

        tags.reverse();
        assert_eq!(sorted_tags(&tags), expected);

        tags.rotate_left(2);
        assert_eq!(sorted_tags(&tags), expected);
    }

    #[test]
    fn sort_releases_large_numbers_up_to_u64_max() {
        assert_eq!(
            sorted_tags(&["v1.0.18446744073709551614", "v1.0.18446744073709551615", "v1.0.4294967296", "v1.0.4294967295"]),
            vec!["v1.0.18446744073709551615", "v1.0.18446744073709551614", "v1.0.4294967296", "v1.0.4294967295"],
        );
    }

    #[test]
    fn sort_releases_multi_digit_prerelease_numbers() {
        assert_eq!(
            sorted_tags(&["v10.20.30-beta.9", "v10.20.30-beta.100", "v10.20.30-beta.10", "v10.20.30", "v10.20.29"]),
            vec!["v10.20.30", "v10.20.30-beta.100", "v10.20.30-beta.10", "v10.20.30-beta.9", "v10.20.29"],
        );
    }

    #[test]
    fn sort_releases_calver_multi_digit_micro() {
        assert_eq!(
            sorted_tags(&["v2026.09.99", "v2026.09.1000", "v2026.09.9", "v2026.09.100", "v2026.10.0"]),
            vec!["v2026.10.0", "v2026.09.1000", "v2026.09.100", "v2026.09.99", "v2026.09.9"],
        );
    }

    #[test]
    fn sort_releases_calver_short_and_long_years() {
        assert_eq!(
            sorted_tags(&["v99.12.5", "v100.01.0", "v26.09.10", "v26.09.9"]),
            vec!["v100.01.0", "v99.12.5", "v26.09.10", "v26.09.9"],
        );
    }

    #[test]
    fn sort_releases_with_package_prefix() {
        let mut releases = ["my-pkg@v1.2.9", "my-pkg@v1.2.100", "my-pkg@v1.2.10"]
            .iter()
            .map(|tag| ReleaseInfo::new(tag, "my-pkg@v", false))
            .collect::<Vec<ReleaseInfo>>();
        sort_releases_desc(&mut releases);

        assert_eq!(
            releases.iter().map(|r| r.tag()).collect::<Vec<String>>(),
            vec!["my-pkg@v1.2.100", "my-pkg@v1.2.10", "my-pkg@v1.2.9"],
        );
    }

    #[test]
    fn parse_version_multi_digit_segments() {
        assert_eq!(parse_version("123.4567.89012"), Some(Version::new(123, 4567, 89012)));
        assert_eq!(parse_version("2026.09.1000"), Some(Version::new(2026, 9, 1000)));
        assert_eq!(parse_version("1.0.18446744073709551615"), Some(Version::new(1, 0, u64::MAX)));
    }

    #[test]
    fn parse_version_rejects_segment_overflowing_u64() {
        // Zero-padded so it takes the CalVer path too; neither parser can hold it
        assert!(parse_version("1.0.18446744073709551616").is_none());
        assert!(parse_version("01.0.18446744073709551616").is_none());
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

    // parse_origin_url tests

    #[test]
    fn parse_origin_url_https() {
        let (host, owner, repo) = parse_origin_url("https://github.com/south-bay-warriors/donder-release").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "donder-release");
    }

    #[test]
    fn parse_origin_url_https_with_git_suffix() {
        let (host, owner, repo) = parse_origin_url("https://github.com/south-bay-warriors/donder-release.git").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "donder-release");
    }

    #[test]
    fn parse_origin_url_ssh() {
        let (host, owner, repo) = parse_origin_url("git@github.com:south-bay-warriors/donder-release.git").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "donder-release");
    }

    #[test]
    fn parse_origin_url_ssh_without_git_suffix() {
        let (host, owner, repo) = parse_origin_url("git@github.com:south-bay-warriors/donder-release").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "donder-release");
    }

    #[test]
    fn parse_origin_url_ssh_repo_with_dots() {
        let (host, owner, repo) = parse_origin_url("git@github.com:south-bay-warriors/com.example.git").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "com.example");
    }

    #[test]
    fn parse_origin_url_ssh_repo_with_subdomain() {
        let (host, owner, repo) = parse_origin_url("git@github.com:south-bay-warriors/com.example.subdomain.git").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "com.example.subdomain");
    }

    #[test]
    fn parse_origin_url_repo_with_dots() {
        let (host, owner, repo) = parse_origin_url("https://github.com/south-bay-warriors/com.example").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "com.example");
    }

    #[test]
    fn parse_origin_url_repo_with_dots_and_git_suffix() {
        let (host, owner, repo) = parse_origin_url("https://github.com/south-bay-warriors/com.example.git").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "com.example");
    }

    #[test]
    fn parse_origin_url_repo_with_subdomain() {
        let (host, owner, repo) = parse_origin_url("https://github.com/south-bay-warriors/com.example.subdomain").unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(owner, "south-bay-warriors");
        assert_eq!(repo, "com.example.subdomain");
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
