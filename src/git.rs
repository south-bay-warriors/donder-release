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
}

impl Git {
    pub fn new(token: &str, author: &str, email: &str) -> Result<Self> {
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
                author: author.to_string(),
                email: email.to_string(),
                owner: caps[4].to_string(),
                repo: caps[5].to_string(),
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

        // pull changes from remote
        Command::new("git")
            .args(["pull", &self.repo_url])
            .output()?;

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
            bail!("failed to push changes token may be invalid");
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
