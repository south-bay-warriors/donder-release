use std::{path, fs};
use anyhow::{Result, bail};
use inquire::{Select, Text, MultiSelect, Confirm};

struct SetupConfig {
    versioning: String,
    calver_format: String,
    tag_prefix: String,
    release_message: String,
    changelog_file: String,
    clean_pre_releases: bool,
    bump_files: Vec<BumpFileEntry>,
}

struct BumpFileEntry {
    target: String,
    path: String,
    build_metadata: bool,
    package: bool,
    versioning: String,
    calver_format: String,
}

pub fn interactive_setup() -> Result<()> {
    let config_path = path::Path::new("./donder-release.yaml");

    if config_path.exists() {
        let overwrite = Confirm::new("donder-release.yaml already exists. Overwrite?")
            .with_default(false)
            .prompt()?;

        if !overwrite {
            bail!("setup cancelled");
        }
    }

    println!("\nWelcome to donder-release setup\n");

    // Versioning scheme
    let versioning = Select::new(
        "Versioning scheme:",
        vec!["semver", "calver"],
    )
    .with_help_message("semver: semantic versioning (1.0.0), calver: calendar versioning (2026.4.0)")
    .prompt()?;

    let calver_format = if versioning == "calver" {
        let format_options = vec![
            ("YYYY.MM.MICRO",  "e.g. 2026.4.0"),
            ("YYYY.0M.MICRO", "e.g. 2026.04.0"),
            ("YY.MM.MICRO",    "e.g. 26.4.0"),
            ("YY.0M.MICRO",   "e.g. 26.04.0"),
            ("YYYY.WW.MICRO",  "e.g. 2026.15.0"),
            ("YYYY.0W.MICRO", "e.g. 2026.15.0"),
        ];

        let labels: Vec<String> = format_options.iter()
            .map(|(fmt, ex)| format!("{} ({})", fmt, ex))
            .collect();

        let selected = Select::new("CalVer format:", labels.clone())
            .with_help_message("MICRO auto-increments per release, 0M/0W are zero-padded")
            .prompt()?;

        // Extract the format from the label
        format_options[labels.iter().position(|l| l == &selected).unwrap()].0.to_string()
    } else {
        String::new()
    };

    // Tag prefix
    let prefix_help = if versioning == "calver" {
        "Prefix for release tags (e.g. v for v2026.4.0, or empty for 2026.4.0)"
    } else {
        "Prefix for release tags (e.g. v for v1.0.0, or empty for 1.0.0)"
    };
    let tag_prefix = Text::new("Tag prefix (leave empty for none):")
        .with_help_message(prefix_help)
        .prompt()?;

    // Release message
    let release_message = Text::new("Release commit message:")
        .with_default("chore(release): %s")
        .with_help_message("Use %s as placeholder for the version")
        .prompt()?;

    // Monorepo
    let is_monorepo = Confirm::new("Is this a monorepo?")
        .with_default(false)
        .with_help_message("Each package gets its own releases based on commits under its directory")
        .prompt()?;

    let mut bump_files = Vec::new();

    if is_monorepo {
        loop {
            let name = Text::new("Package name (leave empty to finish):")
                .with_help_message("e.g. api, web, mobile")
                .prompt()?;

            if name.is_empty() {
                break;
            }

            let target = Select::new(
                &format!("Versioning target for '{}':", name),
                vec!["cargo", "npm", "pub", "android", "ios"],
            ).prompt()?;

            let default_path = match target {
                "android" => format!("packages/{}/android", name),
                "ios" => format!("packages/{}/ios/{}", name, name),
                _ => format!("packages/{}", name),
            };

            let path = Text::new(&format!("Path for '{}' package:", name))
                .with_default(&default_path)
                .prompt()?;

            let build_metadata = match target {
                "android" | "ios" => false,
                _ => {
                    Confirm::new(&format!("Include build metadata for '{}'?", name))
                        .with_default(false)
                        .with_help_message("Appends an auto-incrementing build number (e.g. +1, +2)")
                        .prompt()?
                }
            };

            let use_global = Confirm::new(&format!("Use global versioning ({}) for '{}'?", versioning, name))
                .with_default(true)
                .prompt()?;

            let (pkg_versioning, pkg_calver_format) = if use_global {
                (String::new(), String::new())
            } else {
                let v = Select::new(
                    &format!("Versioning for '{}':", name),
                    vec!["semver", "calver"],
                ).prompt()?;

                let cf = if v == "calver" {
                    let format_options = vec![
                        ("YYYY.MM.MICRO",  "e.g. 2026.4.0"),
                        ("YYYY.0M.MICRO", "e.g. 2026.04.0"),
                        ("YY.MM.MICRO",    "e.g. 26.4.0"),
                        ("YY.0M.MICRO",   "e.g. 26.04.0"),
                        ("YYYY.WW.MICRO",  "e.g. 2026.15.0"),
                        ("YYYY.0W.MICRO", "e.g. 2026.15.0"),
                    ];
                    let labels: Vec<String> = format_options.iter()
                        .map(|(fmt, ex)| format!("{} ({})", fmt, ex))
                        .collect();
                    let selected = Select::new(&format!("CalVer format for '{}':", name), labels.clone()).prompt()?;
                    format_options[labels.iter().position(|l| l == &selected).unwrap()].0.to_string()
                } else {
                    String::new()
                };

                (v.to_string(), cf)
            };

            bump_files.push(BumpFileEntry {
                target: target.to_string(),
                path,
                build_metadata,
                package: true,
                versioning: pkg_versioning,
                calver_format: pkg_calver_format,
            });
        }
    } else {
        let targets = MultiSelect::new(
            "Which versioning files should be bumped?",
            vec!["cargo", "npm", "pub", "android", "ios"],
        )
        .with_help_message("Select one or more targets")
        .prompt()?;

        for target in &targets {
            let default_path = match *target {
                "android" => "android",
                "ios" => "ios/MyApp",
                _ => "<root>",
            };

            let path = Text::new(&format!("Path for {} target:", target))
                .with_default(default_path)
                .with_help_message("Use <root> for the current directory")
                .prompt()?;

            let build_metadata = match *target {
                "android" | "ios" => false,
                _ => {
                    Confirm::new(&format!("Include build metadata for {}?", target))
                        .with_default(false)
                        .with_help_message("Appends an auto-incrementing build number (e.g. +1, +2)")
                        .prompt()?
                }
            };

            bump_files.push(BumpFileEntry {
                target: target.to_string(),
                path,
                build_metadata,
                package: false,
                versioning: String::new(),
                calver_format: String::new(),
            });
        }
    }

    if bump_files.is_empty() {
        bail!("at least one bump file target must be selected");
    }

    // Custom types
    let add_custom_types = Confirm::new("Add custom commit types?")
        .with_default(false)
        .with_help_message("feat, fix and revert are included by default")
        .prompt()?;

    let mut custom_types = Vec::new();
    if add_custom_types {
        loop {
            let commit_type = Text::new("Commit type (leave empty to finish):")
                .with_help_message("e.g. perf, docs, style")
                .prompt()?;

            if commit_type.is_empty() {
                break;
            }

            let bump = Select::new(
                &format!("Bump type for '{}':", commit_type),
                vec!["patch", "minor"],
            ).prompt()?;

            let default_section = format!("{} Changes", commit_type[..1].to_uppercase() + &commit_type[1..]);
            let section = Text::new(&format!("Changelog section title for '{}':", commit_type))
                .with_default(&default_section)
                .prompt()?;

            custom_types.push((commit_type, bump.to_string(), section));
        }
    }

    // Changelog file
    let write_changelog = Confirm::new("Write changelog to file?")
        .with_default(false)
        .prompt()?;

    let changelog_file = if write_changelog {
        Text::new("Changelog file name:")
            .with_default("CHANGELOG.md")
            .prompt()?
    } else {
        String::new()
    };

    // Clean pre-releases (only for semver)
    let clean_pre_releases = if versioning == "semver" {
        Confirm::new("Clean pre-releases when a stable release is published?")
            .with_default(false)
            .with_help_message("Deletes pre-release tags and GitHub releases")
            .prompt()?
    } else {
        false
    };

    // Build YAML
    let config = build_config(SetupConfig {
        versioning: versioning.to_string(),
        calver_format,
        tag_prefix,
        release_message,
        changelog_file,
        clean_pre_releases,
        bump_files,
    }, custom_types);

    fs::write(config_path, config)?;

    println!("\nConfiguration saved to donder-release.yaml");

    Ok(())
}

fn build_config(config: SetupConfig, custom_types: Vec<(String, String, String)>) -> String {
    let mut yaml = String::from("# Configuration file for donder-release\n\n");

    yaml.push_str(&format!("versioning: {}\n", config.versioning));
    if !config.calver_format.is_empty() {
        yaml.push_str(&format!("calver_format: {}\n", config.calver_format));
    }
    yaml.push_str(&format!("tag_prefix: {}\n", config.tag_prefix));
    yaml.push_str(&format!("release_message: \"{}\"\n", config.release_message));

    if !config.changelog_file.is_empty() {
        yaml.push_str(&format!("changelog_file: {}\n", config.changelog_file));
    }

    if config.clean_pre_releases {
        yaml.push_str("clean_pre_releases: true\n");
    }

    // Types
    if !custom_types.is_empty() {
        yaml.push_str("types:\n");
        for (commit_type, bump, section) in &custom_types {
            yaml.push_str(&format!(
                "  - {{ commit_type: {}, bump: {}, section: {} }}\n",
                commit_type, bump, section,
            ));
        }
    }

    // Bump files
    yaml.push_str("bump_files:\n");
    for file in &config.bump_files {
        let mut props = format!("target: {}, path: {}", file.target, file.path);
        if file.build_metadata {
            props.push_str(", build_metadata: true");
        }
        if file.package {
            props.push_str(", package: true");
        }
        if !file.versioning.is_empty() {
            props.push_str(&format!(", versioning: {}", file.versioning));
        }
        if !file.calver_format.is_empty() {
            props.push_str(&format!(", calver_format: {}", file.calver_format));
        }
        yaml.push_str(&format!("  - {{ {} }}\n", props));
    }

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_config_basic() {
        let config = build_config(SetupConfig {
            versioning: "semver".to_string(),
            calver_format: String::new(),
            tag_prefix: "v".to_string(),
            release_message: "chore(release): %s".to_string(),
            changelog_file: String::new(),
            clean_pre_releases: false,
            bump_files: vec![
                BumpFileEntry { target: "cargo".to_string(), path: "<root>".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
            ],
        }, vec![]);

        assert!(config.contains("versioning: semver"));
        assert!(!config.contains("calver_format"));
        assert!(config.contains("tag_prefix: v"));
        assert!(config.contains("release_message: \"chore(release): %s\""));
        assert!(config.contains("- { target: cargo, path: <root> }"));
        assert!(!config.contains("changelog_file"));
        assert!(!config.contains("clean_pre_releases"));
    }

    #[test]
    fn build_config_with_all_options() {
        let config = build_config(SetupConfig {
            versioning: "semver".to_string(),
            calver_format: String::new(),
            tag_prefix: "v".to_string(),
            release_message: "chore(release): %s".to_string(),
            changelog_file: "CHANGELOG.md".to_string(),
            clean_pre_releases: true,
            bump_files: vec![
                BumpFileEntry { target: "cargo".to_string(), path: "<root>".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
                BumpFileEntry { target: "npm".to_string(), path: "<root>".to_string(), build_metadata: true, package: false, versioning: String::new(), calver_format: String::new() },
            ],
        }, vec![
            ("perf".to_string(), "patch".to_string(), "Performance Improvements".to_string()),
        ]);

        assert!(config.contains("changelog_file: CHANGELOG.md"));
        assert!(config.contains("clean_pre_releases: true"));
        assert!(config.contains("- { target: cargo, path: <root> }"));
        assert!(config.contains("- { target: npm, path: <root>, build_metadata: true }"));
        assert!(config.contains("- { commit_type: perf, bump: patch, section: Performance Improvements }"));
    }

    #[test]
    fn build_config_calver() {
        let config = build_config(SetupConfig {
            versioning: "calver".to_string(),
            calver_format: "YYYY.MM.MICRO".to_string(),
            tag_prefix: "v".to_string(),
            release_message: "chore(release): %s".to_string(),
            changelog_file: String::new(),
            clean_pre_releases: false,
            bump_files: vec![
                BumpFileEntry { target: "npm".to_string(), path: "<root>".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
            ],
        }, vec![]);

        assert!(config.contains("versioning: calver"));
        assert!(config.contains("calver_format: YYYY.MM.MICRO"));
    }

    #[test]
    fn build_config_multiple_targets() {
        let config = build_config(SetupConfig {
            versioning: "semver".to_string(),
            calver_format: String::new(),
            tag_prefix: "v".to_string(),
            release_message: "chore(release): %s".to_string(),
            changelog_file: String::new(),
            clean_pre_releases: false,
            bump_files: vec![
                BumpFileEntry { target: "cargo".to_string(), path: "<root>".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
                BumpFileEntry { target: "npm".to_string(), path: "<root>".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
                BumpFileEntry { target: "android".to_string(), path: "android".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
                BumpFileEntry { target: "ios".to_string(), path: "ios/MyApp".to_string(), build_metadata: false, package: false, versioning: String::new(), calver_format: String::new() },
            ],
        }, vec![]);

        assert!(config.contains("- { target: cargo, path: <root> }"));
        assert!(config.contains("- { target: npm, path: <root> }"));
        assert!(config.contains("- { target: android, path: android }"));
        assert!(config.contains("- { target: ios, path: ios/MyApp }"));
    }

    #[test]
    fn build_config_monorepo_packages() {
        let config = build_config(SetupConfig {
            versioning: "semver".to_string(),
            calver_format: String::new(),
            tag_prefix: "v".to_string(),
            release_message: "chore(release): %s".to_string(),
            changelog_file: String::new(),
            clean_pre_releases: false,
            bump_files: vec![
                BumpFileEntry { target: "npm".to_string(), path: "packages/api".to_string(), build_metadata: false, package: true, versioning: String::new(), calver_format: String::new() },
                BumpFileEntry { target: "npm".to_string(), path: "packages/web".to_string(), build_metadata: false, package: true, versioning: String::new(), calver_format: String::new() },
            ],
        }, vec![]);

        assert!(config.contains("- { target: npm, path: packages/api, package: true }"));
        assert!(config.contains("- { target: npm, path: packages/web, package: true }"));
    }

    #[test]
    fn build_config_monorepo_per_package_versioning() {
        let config = build_config(SetupConfig {
            versioning: "semver".to_string(),
            calver_format: String::new(),
            tag_prefix: "v".to_string(),
            release_message: "chore(release): %s".to_string(),
            changelog_file: String::new(),
            clean_pre_releases: false,
            bump_files: vec![
                BumpFileEntry { target: "npm".to_string(), path: "packages/api".to_string(), build_metadata: false, package: true, versioning: "semver".to_string(), calver_format: String::new() },
                BumpFileEntry { target: "npm".to_string(), path: "packages/app".to_string(), build_metadata: false, package: true, versioning: "calver".to_string(), calver_format: "YYYY.MM.MICRO".to_string() },
            ],
        }, vec![]);

        assert!(config.contains("- { target: npm, path: packages/api, package: true, versioning: semver }"));
        assert!(config.contains("- { target: npm, path: packages/app, package: true, versioning: calver, calver_format: YYYY.MM.MICRO }"));
    }
}
