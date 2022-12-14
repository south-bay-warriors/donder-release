use std::{
    path,
    fs,
    collections::HashMap,
};
use anyhow::{Context, Result, bail, Ok};
use serde::Deserialize;

use crate::{
    git::Git,
    api::GithubApi,
    package::{Pkg, BumpFiles}
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
# If defined will bump the version in this files, at least one file must be defined for a release to be published.
# (supported versioning file targets: cargo, npm, pub, android and ios)
# Set the package property to true and the bump file parent folder will be treated as the root for commits made under
# that folder and will have their own releases, this is useful for monorepos.
# bump_files:
#   - { target: cargo, path: Cargo.toml }
#   - { target: npm, path: package.json }
#   - { target: pub, path: pubspec.yaml, build_metadata: true }
#   - { target: android, path: app/build.gradle, build_metadata: true }
#   - { target: ios, path: <my_app>/Info.plist, build_metadata: true }
#   - { target: npm, path: packages/a-test/package.json, package: true }
#   - { target: npm, path: packages/b-test/package.json, package: true }
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
    /// git api
    #[serde(skip)]
    pub git: Git,
    /// github api
    #[serde(skip)]
    pub api: GithubApi,
    // packages to bump
    #[serde(skip)]
    pub packages: Vec<Pkg>,
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

impl Ctx {
    pub fn new(config: String, pre_id: String, preview: bool, selected_packages: Vec<String>) -> Result<Self> {
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

        let mut packages = HashMap::new();

        packages.insert(
            "root".to_string(),
            Pkg::new("".to_string(),
                "".to_string(),
                input_config.tag_prefix.clone(),
                vec![],
            )?,
        );

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

            // Build packages list
            if bump_file.package {
                // get package name from bump file path string
                let segments = bump_file.path.split("/").collect::<Vec<&str>>();

                if segments.len() < 2 {
                    bail!("invalid bump file path for a package");
                }
                
                // package name should be the second to last segment
                let package_name = segments[segments.len() - 2].to_string();

                if !packages.contains_key(&package_name) {
                    packages.insert(
                        package_name.clone(),
                        Pkg::new(
                            package_name.clone(),
                            // join all segments except the last one to get the root path of the package
                            segments[..segments.len() - 1].join("/"),
                            input_config.tag_prefix.clone(),
                            vec![bump_file.clone()],
                        )?,
                    );
                } else {
                    packages.get_mut(&package_name).unwrap().bump_files.push(bump_file.clone());
                }
            } else {
                packages.get_mut("root").unwrap().bump_files.push(bump_file.clone());
            }
        }

        // Remove root package if it has no bump files
        if packages.get("root").unwrap().bump_files.is_empty() {
            packages.remove("root");
        }

        let mut collected_packages: Vec<Pkg> = packages.into_iter().map(|(_, v)| v).collect();

        // If selected packages are provided, filter out the rest
        if !selected_packages.is_empty() {
            collected_packages = collected_packages.into_iter().filter(|pkg| {
                selected_packages.contains(&pkg.name)
            }).collect();
        }
        
        // If no packages are left, bail
        if collected_packages.is_empty() {
            bail!("no packages to release make sure you have selected packages defined in your config file");
        }

        let token = std::env::var("GH_TOKEN").context("GH_TOKEN env var not set")?;

        let git_api = Git::new(
            &token,
            &std::env::var("GIT_AUTHOR_NAME").unwrap_or("cloudoki-deploy".to_string()),
            &std::env::var("GIT_AUTHOR_EMAIL").unwrap_or("opensource@cloudoki.com".to_string()),
        ).context("failed to create git api")?;

        let github_api = GithubApi::new(
            &token,
            &git_api.owner,
            &git_api.repo,
        );

        Ok(
            Self {
                preview,
                pre_id,
                git: git_api,
                api: github_api,
                types: default_types,
                packages: collected_packages,
                ..input_config
            }
        )
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
