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
    git::{Git, ReleaseInfo, Commits},
    api::GithubApi,
    changelog::Changelog,
    bump_files::*,
};

/// Initializes the configuration file
pub fn init_config() -> Result<()> {
    let config = r#"# Configuration file for donder-release

# Release message of the release commit - /%s/ will be replaced with the release version
release_message: "chore(release): %s"
# Prefix of the release tag
tag_prefix: v
# If defined changelog will be written to this file
# changelog_file: CHANGELOG.md
# Allowed types that trigger a release and their corresponding semver bump
# feat, fix and revert commit types are reserved types and can only have its section name changed
# types:
#   - { commit_type: feat, section: Features }
#   - { commit_type: fix, section: Bug Fixes }
#   - { commit_type: perf, bump: patch, section: Performance Improvements }
# If defined will bump the version in this files at least one file must be defined for a release to be published
# (supported versioning file targets: cargo, npm, pub, android and ios)
# bump_files:
#   - { target: cargo, path: Cargo.toml, build_metadata: false }
#   - { target: npm, path: package.json, build_metadata: false }
#   - { target: pub, path: pubspec.yaml, build_metadata: true }
#   - { target: android, path: app/build.gradle, build_metadata: true }
#   - { target: ios, path: <my_app>/Info.plist, build_metadata: true }
"#;

    let config_path = path::Path::new("./donder-release.yaml");

    if config_path.exists() {
        bail!("file already exists");
    }

    fs::write(config_path, config).context("failed to write file")?;

    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct Ctx {
    /// Release message of the release commit
    #[serde(default = "default_release_message")]
    pub release_message: String,
    /// Prefix of the release tag
    #[serde(default = "default_tag_prefix")]
    pub tag_prefix: String,
    /// Allowed types of that trigger a release and their corresponding semver bump
    #[serde(default = "default_types")]
    pub types: ReleaseTypes,
    /// Allowed types of that trigger a release and their corresponding semver bump
    #[serde(default = "default_bump_files")]
    pub bump_files: BumpFiles,
    /// Include authors in changelog
    #[serde(default = "default_include_authors")]
    pub include_authors: bool,
    /// If not empty changelog will be written to this file
    #[serde(default)]
    pub changelog_file: String,
    /// Release optional pre ID (e.g: alpha, beta, rc)
    #[serde(skip)]
    pub pre_id: String,
    /// When in preview mode, the release will not be published.
    #[serde(skip)]
    pub preview: bool,
    /// Last release tag
    #[serde(skip, default = "default_last_release_tag")]
    pub last_release: ReleaseInfo,
    /// Commits since last release
    #[serde(skip, default = "default_commits")]
    pub commits: Commits,
    /// git api
    #[serde(skip)]
    pub git: Git,
    /// github api
    #[serde(skip)]
    pub api: GithubApi,
    /// changelog api
    #[serde(skip)]
    pub changelog: Changelog,
}

fn default_release_message() -> String {
    "chore(release): %s".to_string()
}

fn default_tag_prefix() -> String {
    "v".to_string()
}

fn default_types() -> ReleaseTypes {
    vec![]
}

fn default_bump_files() -> BumpFiles {
    vec![]
}

fn default_include_authors() -> bool {
    true
}

fn default_last_release_tag() -> ReleaseInfo {
    ReleaseInfo::new("0.0.0", "v", false)
}

fn default_commits() -> Commits {
    vec![]
}

impl Ctx {
    pub fn new(config: String, pre_id: String, preview: bool) -> Result<Self> {
        let config_path = path::PathBuf::from(config);
        let file = fs::File::open(config_path).expect("could not open file");
        let input_config: Ctx = serde_yaml::from_reader(file)
            .expect("failed to parse file");
        let mut default_types = vec![
            ReleaseType {
                commit_type: "feat".to_string(),
                bump: "minor".to_string(),
                section: "Features".to_string(),
            },
            ReleaseType {
                commit_type: "fix".to_string(),
                bump: "patch".to_string(),
                section: "Bug Fixes".to_string(),
            },
            ReleaseType {
                commit_type: "revert".to_string(),
                bump: "patch".to_string(),
                section: "Reverts".to_string(),
            },
        ];

        // Parse types
        for release_type in input_config.types {
            // Protect fix, feat and revert types
            if release_type.commit_type == "feat"
                || release_type.commit_type == "fix"
                || release_type.commit_type == "revert"
            {
                if !release_type.bump.is_empty() {
                    bail!("feat, fix and perf are reserved types and cannot have a bump");
                }
            // Only allow minor and patch bumps
            } else if release_type.bump != "minor" && release_type.bump != "patch" {
                bail!("only minor and patch bumps are allowed");
            }

            // Protect type section from being empty
            if release_type.section.is_empty() {
                bail!("type section cannot be empty");
            }

            // Update default types
            match release_type.commit_type.as_str() {
                "feat" => {
                    default_types[0].section = release_type.section;
                },
                "fix" => {
                    default_types[1].section = release_type.section;
                },
                "revert" => {
                    default_types[2].section = release_type.section;
                },
                _ => {
                    default_types.push(release_type);
                }
            }
        }

        // Enforce at least one bump file
        if input_config.bump_files.is_empty() {
            bail!("at least one bump file must be defined");
        }

        // Protect bump files from unsupported targets
        for bump_file in &input_config.bump_files {
            if bump_file.target != "cargo"
                && bump_file.target != "npm"
                && bump_file.target != "pub"
                && bump_file.target != "android"
                && bump_file.target != "ios"
            {
                bail!("unsupported bump file target");
            }
        }

        let token = std::env::var("GH_TOKEN").context("GH_TOKEN env var not set")?;

        let git_api = Git::new(
            &token,
            &std::env::var("GIT_AUTHOR_NAME").unwrap_or("cloudoki-deploy".to_string()),
            &std::env::var("GIT_AUTHOR_EMAIL").unwrap_or("general@cloudoki.com".to_string()),
        ).context("failed to create git api")?;

        let github_api = GithubApi::new(
            &token,
            &git_api.owner,
            &git_api.repo,
        );

        Ok(
            Self {
                pre_id,
                preview,
                git: git_api,
                api: github_api,
                changelog: Changelog::new(),
                types: default_types,
                ..input_config
            }
        )
    }

    pub fn last_release(&mut self) -> Result<()> {
        // self.last_release_tag = self.git.get_last_release_tag(&self.tag_prefix)?;
        let tags = self.git.get_tags(&self.tag_prefix)
            .context("failed to get tags")?;

        for tag in tags {
            if !self.pre_id.is_empty() && tag.version.pre.contains(&self.pre_id) {
                self.last_release = tag;
                break;
            }

            // Default to latest tag
            if tag.version.pre.is_empty() {
                self.last_release = tag;
                break;
            }
        }

        if self.last_release.version == default_last_release_tag().version {
            logInfo!("No previous release found, assuming first release.");

            if !self.pre_id.is_empty() {
                self.last_release = ReleaseInfo::new(&format!("1.0.0-{}.0", self.pre_id), &self.tag_prefix, true)
            } else {
                self.last_release = ReleaseInfo::new("1.0.0", &self.tag_prefix, true)
            }
        } else {
            logInfo!("Last release: {}", self.last_release.version);

            self.last_release.update_head(
                &self.git.tag_head(&self.last_release.tag())
                    .context("failed to get tag head")?
            );
        }
        
        Ok(())
    }

    pub fn get_commits(&mut self) -> Result<()> {
        match &self.last_release.initial {
            true => {
                logInfo!("Retrieving all commits");
                self.commits = self.git.get_commits("")
                    .context("failed to get commits")?;
            },
            false => {
                logInfo!("Retrieving commits since head {}", self.last_release.head);

                 self.commits = self.git.get_commits(&self.last_release.head)
                    .context("failed to get commits")?;
            }
        }

        Ok(())
    }

    pub fn load_changelog(&mut self) -> Result<bool> {
        logInfo!("Analyzing {} commits for changelog", self.commits.len());
        
        // Get a vector of all release types
        let release_types: Vec<String> = self.types
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
            self.changelog.next_release_version = format!("{}{}", self.tag_prefix, Version {
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

            for release_type in &self.types {
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

        if !self.pre_id.is_empty() {
            let mut pre = next_release_version.pre;

            if pre.is_empty() {
                pre = Prerelease::new(format!("{}.0", self.pre_id.clone()).as_str())
                    .context("failed to update pre release")?
            } else {
                let parts = pre.split(".").collect::<Vec<&str>>();

                if parts[0] == self.pre_id {
                    pre = Prerelease::new(
                        format!("{}.{}", self.pre_id.clone(), parts[1].parse::<u32>().unwrap() + 1).as_str(),
                    ).context("failed to update pre release")?
                } else {
                    pre = Prerelease::new(format!("{}.0", self.pre_id.clone()).as_str())
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

        self.changelog.next_release_version = format!("{}{}", self.tag_prefix, next_release_version);

        logInfo!("Next release version: {}", self.changelog.next_release_version);

        Ok(true)
    }

    pub fn write_notes(&mut self) -> Result<()> {
        logInfo!("Writing release notes");

        let origin_url = &self.git.origin_url().context("failed to get git orin url")?;

        self.changelog.write_notes(&self.last_release.tag(), &self.types, origin_url)
            .context("failed to write release notes")?;

        // Write to file if specified and not in preview mode
        if !self.preview && !self.changelog_file.is_empty() {
            let path = path::PathBuf::from(&self.changelog_file);
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

            logInfo!("Wrote release notes to {}", self.changelog_file);
        }

        Ok(())
    }

    pub fn bump_files(&self) -> Result<()> {
        logInfo!("Bumping versioning files");

        let version = &self.changelog.next_release_version.replace(&self.tag_prefix, "");

        for file in &self.bump_files {
            match file.target.as_str() {
                "cargo" => {
                    bump_cargo(version, &file.path, &file.build_metadata)
                        .context("failed to bump cargo file")?;
                },
                "npm" => {
                    bump_npm(version, &file.path, &file.build_metadata)
                        .context("failed to bump npm file")?;
                },
                "pub" => {
                    bump_pub(version, &file.path, &file.build_metadata)
                        .context("failed to bump pub file")?;
                },
                "android" => {
                    bump_android(version, &file.path)
                        .context("failed to bump android file")?;
                },
                "ios" => {
                    bump_ios(version, &file.path)
                        .context("failed to bump ios file")?;
                },
                _ => bail!("invalid file bump target"),
            }
        }

        // Wait a little bit to make sure the files are updated
        thread::sleep(Duration::from_secs(2));

        Ok(())
    }

    pub async fn publish_release(&self) -> Result<()> {
        logInfo!("Publishing release");

        // Release commit
        self.git
            .commit(&self.release_message.replace("%s", &self.changelog.next_release_version))
            .context("failed to commit release")?;

        // Release tag
        self.git.tag(&self.changelog.next_release_version)
            .context("failed to tag release")?;

        // Push to remote
        self.git.push_with_tags()
            .context("failed to push release tag")?;
        
        // Create release on GitHub
        self.api.publish_release(
            &self.changelog.next_release_version,
            &self.tag_prefix,
            &self.changelog.notes)
            .await
            .context("failed to publish release")?;

        Ok(())
    }
}


pub type ReleaseTypes = Vec<ReleaseType>;

#[derive(Debug, Deserialize)]
pub struct ReleaseType {
    /// Type of the commit
    pub commit_type: String,
    /// Corresponding semver bump
    #[serde(default)]
    pub bump: String,
    /// Section of the changelog
    pub section: String,
}

pub type BumpFiles = Vec<BumpFile>;

#[derive(Debug, Deserialize)]
pub struct BumpFile {
    /// Version bump file type (cargo, npm, pub, android and ios)
    pub target: String,
    /// Path to the file that contains the version
    pub path: String,
    /// Include build metadata
    pub build_metadata: bool,
}
