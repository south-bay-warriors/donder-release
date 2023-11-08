use std::{
    path,
    fs,
    io::{Read, Write, Seek, SeekFrom},
    thread,
    time::Duration,
};
use anyhow::{Context, Result, bail, Ok};
use serde::Deserialize;
use chrono::Local;
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
}

impl Pkg {
    pub fn new(name: String, path: String, tag_prefix: String, bump_files: BumpFiles) -> Result<Self> {
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
            }
        )
    }

    pub fn last_release(&mut self, git: &Git, pre_id: &str) -> Result<()> {
        let tags = git.get_tags(&self.tag_prefix)
            .context("failed to get tags")?;

        for tag in tags {
            if !pre_id.is_empty() && tag.version.pre.contains(pre_id) {
                self.last_release = tag;
                break;
            }

            // Default to latest tag
            if tag.version.pre.is_empty() {
                self.last_release = tag;
                break;
            }
        }

        if self.last_release.version == ReleaseInfo::new("0.0.0", "", false).version {
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
            self.changelog.next_release_version = self.last_release.tag();
    
            logInfo!("Next release version: {}", self.changelog.next_release_version);

            return Ok(true)
        }

        // Find next release version
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
                pre = Prerelease::new(format!("{}.0", pre_id.clone()).as_str())
                    .context("failed to update pre release")?
            } else {
                let parts = pre.split(".").collect::<Vec<&str>>();

                if parts[0] == pre_id {
                    pre = Prerelease::new(
                        format!("{}.{}", pre_id.clone(), parts[1].parse::<u32>().unwrap() + 1).as_str(),
                    ).context("failed to update pre release")?
                } else {
                    pre = Prerelease::new(format!("{}.0", pre_id.clone()).as_str())
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

        for file in &self.bump_files {
            match file.target.as_str() {
                "cargo" => {
                    bump_cargo(version, &file.path, &file.build_metadata)?;
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
                    bump_ios(version, &file.path)?;
                },
                _ => bail!("invalid file bump target"),
            }
        }

        // Wait a little bit to make sure the files are updated
        thread::sleep(Duration::from_secs(2));

        Ok(())
    }

    pub async fn publish_release(&self, git: &Git, api: &GithubApi, release_message: &str) -> Result<()> {
        logInfo!("Publishing release");

        // Release commit
        git
            .commit(release_message.replace("%s", &self.changelog.next_release_version).as_str())?;

        // Push to remote
        git.push()?;

        // Release tag
        git.tag(&self.changelog.next_release_version)?;
        git.push_tag(&self.changelog.next_release_version)?;

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
    /// Is this an  individual package that should be published separately
    #[serde(default = "default_package")]
    pub package: bool,
}

fn default_build_metadata() -> bool {
    false
}

fn default_package() -> bool {
    false
}
