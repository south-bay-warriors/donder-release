use std::{
    path,
    fs,
    io::{Read, Write, Seek, SeekFrom},
    thread,
    time::Duration,
};
use anyhow::{Context, Result, bail, Ok};
use serde::Deserialize;
use chrono::{Local, Utc, Datelike};
use semver::{Version, Prerelease, BuildMetadata};

use crate::{
    git::{ReleaseInfo, Commits, Git},
    bump_files::*,
    changelog::Changelog, api::GithubApi, ctx::ReleaseTypes,
};

#[derive(Debug)]
pub struct Pkg {
    /// Package name
    pub name: String,
    /// Package path
    pub path: String,
    /// files to bump
    pub bump_files: BumpFiles,
    /// Last release tag
    pub last_release: ReleaseInfo,
    /// changelog api
    pub changelog: Changelog,
    /// Commits since last release
    pub commits: Commits,
    // Combination of package name and context tag_prefix
    pub tag_prefix: String,
    /// Versioning scheme for this package
    pub versioning: String,
    /// CalVer format for this package
    pub calver_format: String,
}

impl Pkg {
    pub fn new(name: String, path: String, tag_prefix: String, bump_files: BumpFiles, versioning: String, calver_format: String) -> Result<Self> {
        Ok(
            Self {
                tag_prefix: match name.is_empty() {
                    true => tag_prefix,
                    false => format!("{}@{}", name, tag_prefix),
                },
                name,
                path,
                bump_files,
                last_release: ReleaseInfo::new("0.0.0", "", false),
                changelog: Changelog::new(),
                commits: Commits::new(),
                versioning,
                calver_format,
            }
        )
    }

    pub fn last_release(&mut self, git: &Git, pre_id: &str) -> Result<()> {
        let tags = git.get_tags(&self.tag_prefix)
            .context("failed to get tags")?;

        for tag in tags {
            if !pre_id.is_empty() && tag.version_str.contains(pre_id) {
                self.last_release = tag;
                break;
            }

            // Default to latest tag (no pre-release suffix)
            if !tag.version_str.contains('-') {
                self.last_release = tag;
                break;
            }
        }

        if self.last_release.version_str == "0.0.0" {
            logInfo!("No previous release found, assuming first release.");

            if !pre_id.is_empty() {
                self.last_release = ReleaseInfo::new(&format!("1.0.0-{}.0", pre_id), &self.tag_prefix, true)
            } else {
                self.last_release = ReleaseInfo::new("1.0.0", &self.tag_prefix, true)
            }
        } else {
            logInfo!("Last release: {}", self.last_release.tag());

            self.last_release.update_head(
                git.tag_head(&self.last_release.tag())
                    .context("failed to get tag head")?
                    .as_str()
            );
        }
        
        Ok(())
    }

    pub fn get_commits(&mut self, git: &Git) -> Result<()> {
        match &self.last_release.initial {
            true => {
                logInfo!("Retrieving all commits");
                self.commits = git.get_commits("", &self.path)
                    .context("failed to get commits")?;
            },
            false => {
                logInfo!("Retrieving commits since head {}", self.last_release.head);

                 self.commits = git.get_commits(&self.last_release.head, &self.path)
                    .context("failed to get commits")?;
            }
        }

        Ok(())
    }

    pub fn load_changelog(&mut self, pre_id: &str, types: &ReleaseTypes) -> Result<bool> {
        let versioning = &self.versioning;
        let calver_format = &self.calver_format;
        logInfo!("Analyzing {} commits for changelog", self.commits.len());

        // Get a vector of all release types
        let release_types: Vec<String> = types
            .iter()
            .map(|t| t.commit_type.clone())
            .collect();

        // Parse commits
        for commit in &self.commits {
            self.changelog.parse_commit(&release_types, commit)
        }

        if self.changelog.commits.is_empty() {
            logInfo!("No relevant commits found, skipping release");
            return Ok(false)
        }

        logInfo!("Found {} relevant commits", self.changelog.commits.len());

        // We already have the next release tag
        if self.last_release.initial {
            if versioning == "calver" {
                self.changelog.next_release_version = format!(
                    "{}{}",
                    &self.tag_prefix,
                    calver_version(calver_format, 0),
                );
            } else {
                self.changelog.next_release_version = self.last_release.tag();
            }

            logInfo!("Next release version: {}", self.changelog.next_release_version);

            return Ok(true)
        }

        if versioning == "calver" {
            let next_version = self.calver_next_version(calver_format)?;
            self.changelog.next_release_version = format!("{}{}", &self.tag_prefix, next_version);

            logInfo!("Next release version: {}", self.changelog.next_release_version);

            return Ok(true)
        }

        // Semver version calculation
        let mut next_release = self.last_release.tag();
        let mut next_release_type = "patch".to_string();

        // Get next release version
        if next_release.is_empty() {
            self.changelog.next_release_version = format!("{}{}", &self.tag_prefix, Version {
                major: 1,
                minor: 0,
                patch: 0,
                pre: Prerelease::EMPTY,
                build: BuildMetadata::EMPTY,
            });

            logInfo!("Next release version: {}", self.changelog.next_release_version);

            return Ok(true)
        }

        next_release = next_release.replace(&self.tag_prefix, "");

        // Get next release type
        for commit in &self.changelog.commits {
            if !commit.breaking.is_empty() {
                next_release_type = "major".to_string();
                break;
            }

            for release_type in types {
                if commit.section_type == release_type.commit_type && release_type.bump == "minor" {
                    next_release_type = "minor".to_string();
                    break;
                }
            }
        }

        // Get next release version
        let mut next_release_version = semver::Version::parse(&next_release)
            .context("failed to parse next release version")?;

        if next_release_version.pre.is_empty() {
            next_release_version = match next_release_type.as_str() {
                "major" => Version {
                    major: next_release_version.major + 1,
                    minor: 0,
                    patch: 0,
                    pre: Prerelease::EMPTY,
                    build: BuildMetadata::EMPTY,
                },
                "minor" => Version {
                    major: next_release_version.major,
                    minor: next_release_version.minor + 1,
                    patch: 0,
                    pre: Prerelease::EMPTY,
                    build: BuildMetadata::EMPTY,
                },
                "patch" => Version {
                    major: next_release_version.major,
                    minor: next_release_version.minor,
                    patch: next_release_version.patch + 1,
                    pre: Prerelease::EMPTY,
                    build: BuildMetadata::EMPTY,
                },
                _ => bail!("invalid release type"),
            };
        }

        if !pre_id.is_empty() {
            let mut pre = next_release_version.pre;

            if pre.is_empty() {
                pre = Prerelease::new(format!("{}.0", pre_id).as_str())
                    .context("failed to update pre release")?
            } else {
                let parts = pre.split(".").collect::<Vec<&str>>();

                if parts[0] == pre_id {
                    pre = Prerelease::new(
                        format!("{}.{}", pre_id, parts[1].parse::<u32>().unwrap() + 1).as_str(),
                    ).context("failed to update pre release")?
                } else {
                    pre = Prerelease::new(format!("{}.0", pre_id).as_str())
                        .context("failed to update pre release")?
                }
            }

            next_release_version = Version {
                major: next_release_version.major,
                minor: next_release_version.minor,
                patch: next_release_version.patch,
                pre,
                build: BuildMetadata::EMPTY,
            };
        }

        self.changelog.next_release_version = format!("{}{}", &self.tag_prefix, next_release_version);

        logInfo!("Next release version: {}", self.changelog.next_release_version);

        Ok(true)
    }

    fn calver_next_version(&self, format: &str) -> Result<String> {
        let current_calver = calver_version(format, 0);
        let last_version = self.last_release.tag().replace(&self.tag_prefix, "");

        // Check if last release has the same calendar prefix
        let current_parts: Vec<&str> = current_calver.split('.').collect();
        let last_parts: Vec<&str> = last_version.split('.').collect();

        // Find MICRO position in format
        let format_segments: Vec<&str> = format.split('.').collect();
        let micro_idx = format_segments.iter().position(|s| *s == "MICRO").unwrap();

        // Compare non-MICRO segments
        let same_prefix = current_parts.iter().enumerate()
            .filter(|(i, _)| *i != micro_idx)
            .zip(last_parts.iter().enumerate().filter(|(i, _)| *i != micro_idx))
            .all(|((_, a), (_, b))| a == b);

        if same_prefix && last_parts.len() == 3 {
            // Same calendar period: increment MICRO
            let last_micro: u64 = last_parts[micro_idx].parse().unwrap_or(0);
            Ok(calver_version(format, last_micro + 1))
        } else {
            // New calendar period: reset MICRO to 0
            Ok(current_calver)
        }
    }

    pub fn write_notes(&mut self, preview: &bool, git: &Git, types: &ReleaseTypes, changelog_file: &str) -> Result<()> {
        logInfo!("Writing release notes");

        let origin_url = git.origin_url().context("failed to get git orin url")?;

        self.changelog.write_notes(
            &self.last_release.tag(),
            types,
            origin_url.as_str(),
        ).context("failed to write release notes")?;

        // Write to file if specified and not in preview mode
        if !preview && !changelog_file.is_empty() {
            let changelog_file_with_root = match !self.path.is_empty() {
                true => format!("{}/{}", self.path, changelog_file),
                false => changelog_file.to_string(),
            };
            let path = path::PathBuf::from(&changelog_file_with_root);
            let changelog_title = "# CHANGELOG\r\n\r\n_This file is auto-generated by donder-release and should not be edited manually._\r\n\r\n";

            // Check if changelog file exists on disk
            if path.exists() {
                // Write notes after changelog title and before first release
                let mut file = fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .context("failed to open changelog file")?;

                let mut contents = String::new();
                file.read_to_string(&mut contents)
                    .context("failed to read changelog file")?;

                let lines = contents.lines().collect::<Vec<&str>>();
                let mut new_contents = format!("{}{}", changelog_title, self.changelog.notes);

                // Add remaining lines to new contents
                for (i, line) in lines.iter().enumerate() {
                    // Skip first 3 lines (changelog title, description and empty line)
                    if i > 2 {
                        // Write old lines back to new contents
                        new_contents = format!("{}\r\n{}", new_contents, line);
                    }
                }

                // New line at end of file
                new_contents = format!("{}\r\n", new_contents);

                file.set_len(0)
                    .context("failed to truncate changelog file")?;

                file.seek(SeekFrom::Start(0))
                    .context("failed to seek to start of changelog file")?;

                file.write_all(new_contents.as_bytes())
                    .context("failed to write to changelog file")?;
            } else {
                // Create new changelog file
                fs::File::create(&path)
                    .context("failed to create changelog file")?;

                let changelog_content = format!(
                   "{}{}",
                    changelog_title,
                    self.changelog.notes,
                );

                fs::write(path, changelog_content)
                    .context("failed to write to changelog file")?;
            }

            logInfo!("Wrote release notes to {}", changelog_file_with_root);
        }

        Ok(())
    }

    pub fn bump_files(&self) -> Result<()> {
        logInfo!("Bumping versioning files");

        let version = &self.changelog.next_release_version.replace(&self.tag_prefix, "");
        let is_calver = self.versioning == "calver";

        for file in &self.bump_files {
            match file.target.as_str() {
                "cargo" => {
                    bump_cargo(version, &file.path, &file.build_metadata)?;
                    // Update Cargo.lock so it's included in the release commit
                    std::process::Command::new("cargo")
                        .args(["update", "--workspace"])
                        .output()?;
                },
                "npm" => {
                    bump_npm(version, &file.path, &file.build_metadata)?;
                },
                "pub" => {
                    bump_pub(version, &file.path, &file.build_metadata)?;
                },
                "android" => {
                    bump_android(version, &file.path)?;
                },
                "ios" => {
                    bump_ios(version, &file.path, is_calver)?;
                },
                _ => bail!("invalid file bump target"),
            }
        }

        // Wait a little bit to make sure the files are updated
        thread::sleep(Duration::from_secs(2));

        Ok(())
    }

    /// Fails if the next release tag already exists, before any file is bumped or committed
    pub fn ensure_tag_available(&self, git: &Git) -> Result<()> {
        let tag = &self.changelog.next_release_version;

        if git.tag_exists(tag)? {
            bail!("tag {} already exists", tag);
        }

        Ok(())
    }

    pub async fn publish_release(&self, git: &Git, api: &GithubApi, release_message: &str) -> Result<()> {
        logInfo!("Publishing release");

        // Release commit
        git
            .commit(release_message.replace("%s", &self.changelog.next_release_version).as_str())?;

        // Release tag, dropping the release commit if the tag can't be created
        if let Err(e) = git.tag(&self.changelog.next_release_version) {
            git.undo_commit()?;
            return Err(e);
        }

        // Push commit and tag to remote atomically
        git.push_release(&self.changelog.next_release_version)?;

        // Create release on GitHub
        api.publish_release(
            &self.changelog.next_release_version,
            &self.tag_prefix,
            &self.changelog.notes)
            .await?;
        Ok(())
    }

    pub async fn clean_pre_releases(&self, git: &Git, api: &GithubApi) -> Result<()> {
        logInfo!("Cleaning pre releases");

        // Clean pre releases first
        api.clean_pre_releases(&self.tag_prefix).await?;

        // TODO: revise this loop because it can become expensive as the number of tags increases
        // Delete tags
        for tag_info in git.get_tags(&self.tag_prefix)? {
            if tag_info.version.pre.is_empty() {
                continue;
            }

            // Local tag
            git.undo_tag(&tag_info.tag())?;
            // Remote tag
            git.delete_tag(&tag_info.tag())?;
        }

        Ok(())
    }
}

/// Generates a CalVer version string from a format and MICRO value
fn calver_version(format: &str, micro: u64) -> String {
    let now = Utc::now();
    let segments: Vec<&str> = format.split('.').collect();

    segments.iter().map(|seg| {
        match *seg {
            "YYYY" => now.year().to_string(),
            "YY" => (now.year() % 100).to_string(),
            "0Y" => format!("{:02}", now.year() % 100),
            "MM" => now.month().to_string(),
            "0M" => format!("{:02}", now.month()),
            "WW" => now.iso_week().week().to_string(),
            "0W" => format!("{:02}", now.iso_week().week()),
            "MICRO" => micro.to_string(),
            _ => seg.to_string(),
        }
    }).collect::<Vec<String>>().join(".")
}

pub type BumpFiles = Vec<BumpFile>;

#[derive(Debug, Deserialize, Clone)]
pub struct BumpFile {
    /// Version bump file type (cargo, npm, pub, android and ios)
    pub target: String,
    /// Path to the file that contains the version
    pub path: String,
    /// Include build metadata
    #[serde(default = "default_build_metadata")]
    pub build_metadata: bool,
    /// Is this an individual package that should be published separately
    #[serde(default = "default_package")]
    pub package: bool,
    /// Per-package versioning override (semver or calver)
    #[serde(default)]
    pub versioning: String,
    /// Per-package CalVer format override
    #[serde(default)]
    pub calver_format: String,
}

fn default_build_metadata() -> bool {
    false
}

fn default_package() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::ReleaseType;

    fn test_types() -> ReleaseTypes {
        vec![
            ReleaseType { commit_type: "feat".to_string(), bump: "minor".to_string(), section: "Features".to_string() },
            ReleaseType { commit_type: "fix".to_string(), bump: "patch".to_string(), section: "Bug Fixes".to_string() },
            ReleaseType { commit_type: "revert".to_string(), bump: "patch".to_string(), section: "Reverts".to_string() },
        ]
    }

    fn make_pkg(tag: &str, prefix: &str, initial: bool) -> Pkg {
        Pkg {
            name: "".to_string(),
            path: "".to_string(),
            bump_files: vec![],
            last_release: ReleaseInfo::new(tag, prefix, initial),
            changelog: Changelog::new(),
            commits: vec![],
            tag_prefix: prefix.to_string(),
            versioning: "semver".to_string(),
            calver_format: String::new(),
        }
    }

    fn make_calver_pkg(tag: &str, prefix: &str, initial: bool, format: &str) -> Pkg {
        Pkg {
            name: "".to_string(),
            path: "".to_string(),
            bump_files: vec![],
            last_release: ReleaseInfo::new(tag, prefix, initial),
            changelog: Changelog::new(),
            commits: vec![],
            tag_prefix: prefix.to_string(),
            versioning: "calver".to_string(),
            calver_format: format.to_string(),
        }
    }

    fn commit(subject: &str, body: &str) -> crate::git::Commit {
        crate::git::Commit::new("abc123", subject, body)
    }

    // Pkg::new tests

    #[test]
    fn pkg_new_root_tag_prefix() {
        let pkg = Pkg::new("".to_string(), "".to_string(), "v".to_string(), vec![], "semver".to_string(), "".to_string()).unwrap();
        assert_eq!(pkg.tag_prefix, "v");
    }

    #[test]
    fn pkg_new_named_package_tag_prefix() {
        let pkg = Pkg::new("my-lib".to_string(), "packages/my-lib".to_string(), "v".to_string(), vec![], "semver".to_string(), "".to_string()).unwrap();
        assert_eq!(pkg.tag_prefix, "my-lib@v");
    }

    // load_changelog tests

    #[test]
    fn load_changelog_no_relevant_commits_returns_false() {
        let mut pkg = make_pkg("v1.0.0", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("docs: update readme", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(!result);
    }

    #[test]
    fn load_changelog_patch_bump() {
        let mut pkg = make_pkg("v1.0.0", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: resolve crash", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.0.1");
    }

    #[test]
    fn load_changelog_minor_bump() {
        let mut pkg = make_pkg("v1.0.0", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![
            commit("fix: a fix", ""),
            commit("feat: new feature", ""),
        ];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.1.0");
    }

    #[test]
    fn load_changelog_major_bump() {
        let mut pkg = make_pkg("v1.2.3", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("feat: new api", "BREAKING CHANGE: old api removed")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v2.0.0");
    }

    #[test]
    fn load_changelog_prerelease_from_stable() {
        let mut pkg = make_pkg("v1.0.0", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: a fix", "")];

        let result = pkg.load_changelog("alpha", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.0.1-alpha.0");
    }

    #[test]
    fn load_changelog_prerelease_increment() {
        let mut pkg = make_pkg("v1.0.1-alpha.0", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: another fix", "")];

        let result = pkg.load_changelog("alpha", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.0.1-alpha.1");
    }

    #[test]
    fn load_changelog_prerelease_id_change() {
        let mut pkg = make_pkg("v1.0.1-beta.2", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: final fix", "")];

        let result = pkg.load_changelog("rc", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.0.1-rc.0");
    }

    #[test]
    fn load_changelog_initial_release() {
        let mut pkg = make_pkg("v1.0.0", "v", true);
        pkg.commits = vec![commit("feat: initial", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.0.0");
    }

    #[test]
    fn load_changelog_initial_release_prerelease() {
        let mut pkg = make_pkg("v1.0.0-alpha.0", "v", true);
        pkg.commits = vec![commit("feat: initial", "")];

        let result = pkg.load_changelog("alpha", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.0.0-alpha.0");
    }

    // CalVer tests

    #[test]
    fn calver_first_release() {
        let mut pkg = make_calver_pkg("v0.0.0", "v", true, "YYYY.MM.MICRO");
        pkg.commits = vec![commit("feat: initial", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);

        let now = chrono::Utc::now();
        let expected = format!("v{}.{}.0", now.year(), now.month());
        assert_eq!(pkg.changelog.next_release_version, expected);
    }

    #[test]
    fn calver_increments_micro_same_month() {
        let now = chrono::Utc::now();
        let last_tag = format!("v{}.{}.2", now.year(), now.month());
        let mut pkg = make_calver_pkg(&last_tag, "v", false, "YYYY.MM.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: a fix", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);

        let expected = format!("v{}.{}.3", now.year(), now.month());
        assert_eq!(pkg.changelog.next_release_version, expected);
    }

    #[test]
    fn calver_resets_micro_new_month() {
        let now = chrono::Utc::now();
        let prev_month = if now.month() == 1 { 12 } else { now.month() - 1 };
        let prev_year = if now.month() == 1 { now.year() - 1 } else { now.year() };
        let last_tag = format!("v{}.{}.5", prev_year, prev_month);
        let mut pkg = make_calver_pkg(&last_tag, "v", false, "YYYY.MM.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("feat: new feature", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);

        let expected = format!("v{}.{}.0", now.year(), now.month());
        assert_eq!(pkg.changelog.next_release_version, expected);
    }

    #[test]
    fn calver_no_commits_returns_false() {
        let mut pkg = make_calver_pkg("v2026.4.0", "v", false, "YYYY.MM.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("docs: update readme", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(!result);
    }

    #[test]
    fn calver_breaking_change_does_not_affect_version() {
        let now = chrono::Utc::now();
        let last_tag = format!("v{}.{}.0", now.year(), now.month());
        let mut pkg = make_calver_pkg(&last_tag, "v", false, "YYYY.MM.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("feat: new api", "BREAKING CHANGE: old api removed")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);

        let expected = format!("v{}.{}.1", now.year(), now.month());
        assert_eq!(pkg.changelog.next_release_version, expected);
    }

    #[test]
    fn calver_version_helper() {
        let now = chrono::Utc::now();
        let v = calver_version("YYYY.MM.MICRO", 3);
        assert_eq!(v, format!("{}.{}.3", now.year(), now.month()));
    }

    #[test]
    fn calver_version_helper_zero_padded() {
        let now = chrono::Utc::now();
        let v = calver_version("YYYY.0M.MICRO", 0);
        assert_eq!(v, format!("{}.{:02}.0", now.year(), now.month()));
    }

    #[test]
    fn calver_version_helper_short_year() {
        let now = chrono::Utc::now();
        let v = calver_version("YY.MM.MICRO", 5);
        assert_eq!(v, format!("{}.{}.5", now.year() % 100, now.month()));
    }

    #[test]
    fn calver_feat_does_not_trigger_minor() {
        let now = chrono::Utc::now();
        let last_tag = format!("v{}.{}.0", now.year(), now.month());
        let mut pkg = make_calver_pkg(&last_tag, "v", false, "YYYY.MM.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("feat: big new feature", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);

        // CalVer ignores commit type bumps, just increments MICRO
        let expected = format!("v{}.{}.1", now.year(), now.month());
        assert_eq!(pkg.changelog.next_release_version, expected);
    }

    #[test]
    fn calver_multiple_releases_same_period() {
        let now = chrono::Utc::now();

        // First release this month
        let last_tag = format!("v{}.{}.3", now.year(), now.month());
        let mut pkg = make_calver_pkg(&last_tag, "v", false, "YYYY.MM.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: first", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, format!("v{}.{}.4", now.year(), now.month()));
    }

    #[test]
    fn semver_pkg_ignores_global_calver() {
        // A package with semver versioning should use semver even if global is calver
        let mut pkg = make_pkg("v1.0.0", "v", false);
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("feat: new feature", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);
        assert_eq!(pkg.changelog.next_release_version, "v1.1.0");
    }

    #[test]
    fn calver_zero_padded_format_full_flow() {
        let now = chrono::Utc::now();
        let last_tag = format!("v{}.{:02}.1", now.year(), now.month());
        let mut pkg = make_calver_pkg(&last_tag, "v", false, "YYYY.0M.MICRO");
        pkg.last_release.update_head("somehead");
        pkg.commits = vec![commit("fix: a fix", "")];

        let result = pkg.load_changelog("", &test_types()).unwrap();
        assert!(result);

        let expected = format!("v{}.{:02}.2", now.year(), now.month());
        assert_eq!(pkg.changelog.next_release_version, expected);
    }
}
