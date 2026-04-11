use crate::{
    git::Commit,
    ctx::{ReleaseType, ReleaseTypes},
};
use anyhow::{Result, Ok};
use regex::Regex;
use chrono::Utc;

#[derive(Debug, Default)]
pub struct Changelog {
    pub commits: Vec<ChangelogCommit>,
    pub next_release_version: String,
    pub notes: String,
}

#[derive(Debug, Clone)]
pub struct ChangelogCommit {
    pub section_type: String,
    pub scope: String,
    pub desc: String,
    pub breaking: String,
    pub hash: String,
}

impl Changelog {
    pub fn new() -> Self {
        Self {
            commits: Vec::new(),
            next_release_version: "0.0.0".to_string(),
            notes: "".to_string(),
        }
    }

    pub fn parse_commit(&mut self, release_types: &Vec<String>, git_commit: &Commit) {
        let mut commit = ChangelogCommit{
            section_type: String::new(),
            scope: String::new(),
            desc: String::new(),
            breaking: String::new(),
            hash: git_commit.hash.clone(),
        };

        // save a reference to the first line to be used later if needed
        let pattern = r"^(TOKENS){1}(\([\w\-\./]+\))?(!)?: (.+)";
        let pattern = pattern.replace(
            "TOKENS",
            release_types.join("|").as_str(),
        );
        let re = Regex::new(&pattern).unwrap();
        let caps = re.captures(&git_commit.subject);

        match caps {
            Some(caps) => {
                commit.section_type = caps[1].to_string();
                match caps.get(2) {
                    Some(s) => {
                        commit.scope = s.as_str().trim_matches(|c| c == '(' || c == ')').to_string();
                    },
                    None => (),
                }
                match caps.get(4) {
                    Some(d) => commit.desc = d.as_str().to_string(),
                    None => (),
                }
            },
            None => (),
        }

        // Parse commit body
        for line in git_commit.body.lines() {
            // Breaking changes
            if line.starts_with("BREAKING CHANGE: ") {
                commit.breaking = line.replace("BREAKING CHANGE: ", "").to_string();

                // Get commit info if no section type is found, this can happen if the commit
                // is not in the range of release_types but it's still relevant for the changelog
                // because it contains a breaking change, which should trigger a major release.
                if commit.section_type.is_empty() {
                    let re = Regex::new(r"^(\w+)(\([\w\-\./]+\))?(!)?: (.+)").unwrap();
                    let caps = re.captures(&git_commit.subject);
                    match caps {
                        Some(caps) => {
                            commit.section_type = caps[1].to_string();
                            match caps.get(2) {
                                Some(s) => {
                                    commit.scope = s.as_str()
                                        .trim_matches(|c| c == '(' || c == ')')
                                        .to_string();
                                },
                                None => (),
                            }
                            match caps.get(4) {
                                Some(d) => commit.desc = d.as_str().to_string(),
                                None => (),
                            }
                        },
                        None => (),
                    }
                }
            }

            // Footers
            // TODO: Add support for multiple footers
        }

        // Ignore commits without section type
        if !commit.section_type.is_empty() {
            self.commits.push(commit);
        }
    }

    pub fn write_notes(&mut self, last_release_version: &String, release_types: &ReleaseTypes, origin_url: &str) -> Result<()> {
        // Clean notes just in case
        self.notes = String::new();

        // Write header
        if last_release_version.is_empty() {
            self.notes.push_str(&format!("## {}\r\n\r\n", self.next_release_version));
        } else {
            self.notes.push_str(&format!(
                "## [{}]({}/compare/{}...{})\r\n\r\n",
                self.next_release_version,
                &origin_url,
                last_release_version,
                self.next_release_version,
            ));
        }
        self.notes.push_str(&format!("###### _{}_\r\n", Utc::now().format("%b %_d, %Y").to_string()));

        // Group commits by section type in a tuple and push commits to a vector if section type already exists
        let mut sections: Vec<(String, String, Vec<ChangelogCommit>)> = Vec::new();
        for commit in &self.commits {
            let mut found = false;

            // Find section to push new commit
            for (section_type, _, commits) in sections.iter_mut() {
                if section_type == &commit.section_type {
                    commits.push(commit.clone());
                    found = true;
                    break;
                }
            }

            // Section not found so create a new one
            if !found {
                let section_type = commit.section_type.clone();
                // Find section title from release_types section_type
                let section_title = release_types
                    .iter()
                    .find(|r| r.commit_type == section_type)
                    // If section_type is not found in release_types, use section_type as title
                    // This can happen if the commit is not in the range of release_types but it's
                    // still relevant for the changelog because it contains a breaking change, which
                    // should trigger a major release.
                    .unwrap_or(&ReleaseType {
                        commit_type: section_type.clone(),
                        bump: "".to_string(),
                        section: section_type.clone(),
                    })
                    .section
                    .clone();

                // Create new section
                sections.push((section_type, section_title, vec![commit.clone()]));
            }
        }

        // Sort sections in the order of release_types
        sections.sort_by(|a, b| {
            release_types
                .iter()
                .position(|r| r.commit_type == a.0)
                .cmp(&release_types.iter().position(|r| r.commit_type == b.0))
        });

        // Write sections
        for (_, section_title, commits) in sections {
            // Write section title
            self.notes.push_str(&format!("\r\n### {}\r\n", section_title));

            // Group commits by scope
            let mut scopes: Vec<(String, Vec<ChangelogCommit>)> = Vec::new();
            for commit in commits {
                let mut found = false;

                // Find scope to push new commit
                for (scope, commits) in scopes.iter_mut() {
                    if scope == &commit.scope {
                        commits.push(commit.clone());
                        found = true;
                        break;
                    }
                }

                // Scope not found so create a new one
                if !found {
                    // Create new scope
                    scopes.push((commit.scope.clone(), vec![commit.clone()]));
                }
            }

            // Write section commits grouped by scope
            for (scope, commits) in scopes {
                // Write scope
                if !scope.is_empty() {
                    self.notes.push_str(&format!("\r\n- **{}:**\r\n", scope));
                }

                for commit in commits {
                    // Write commit
                    match scope.is_empty() {
                        true => self.notes.push_str(&format!(
                            "- {} ([{}]({}/commit/{}))\r\n",
                            commit.desc,
                            commit.hash,
                            &origin_url,
                            commit.hash,
                        )),
                        false => self.notes.push_str(&format!(
                            "  - {} ([{}]({}/commit/{}))\r\n",
                            commit.desc,
                            commit.hash,
                            &origin_url,
                            commit.hash,
                        )),
                    }
                }
            }
        }

        // filter commits with breaking changes
        let breaking_changes: Vec<&ChangelogCommit> = self
            .commits
            .iter()
            .filter(|c| !c.breaking.is_empty())
            .collect();

        // Write breaking changes section
        if !breaking_changes.is_empty() {
            self.notes.push_str("\r\n### BREAKING CHANGES\r\n");
            for commit in breaking_changes {
                self.notes.push_str(&format!("- {}\r\n", commit.breaking));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_types() -> Vec<String> {
        vec!["feat".to_string(), "fix".to_string(), "revert".to_string(), "perf".to_string()]
    }

    fn default_types() -> ReleaseTypes {
        vec![
            ReleaseType { commit_type: "feat".to_string(), bump: "minor".to_string(), section: "Features".to_string() },
            ReleaseType { commit_type: "fix".to_string(), bump: "patch".to_string(), section: "Bug Fixes".to_string() },
            ReleaseType { commit_type: "revert".to_string(), bump: "patch".to_string(), section: "Reverts".to_string() },
            ReleaseType { commit_type: "perf".to_string(), bump: "patch".to_string(), section: "Performance Improvements".to_string() },
        ]
    }

    // parse_commit tests

    #[test]
    fn parse_commit_feat() {
        let mut cl = Changelog::new();
        let commit = Commit::new("abc123", "feat: add login", "");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].section_type, "feat");
        assert_eq!(cl.commits[0].desc, "add login");
        assert!(cl.commits[0].scope.is_empty());
        assert!(cl.commits[0].breaking.is_empty());
    }

    #[test]
    fn parse_commit_fix_with_scope() {
        let mut cl = Changelog::new();
        let commit = Commit::new("def456", "fix(auth): handle null token", "");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].section_type, "fix");
        assert_eq!(cl.commits[0].scope, "auth");
        assert_eq!(cl.commits[0].desc, "handle null token");
    }

    #[test]
    fn parse_commit_breaking_change_in_body() {
        let mut cl = Changelog::new();
        let commit = Commit::new("ghi789", "feat: new api", "BREAKING CHANGE: removed old endpoint");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].breaking, "removed old endpoint");
    }

    #[test]
    fn parse_commit_breaking_change_unknown_type_gets_parsed() {
        let mut cl = Changelog::new();
        let commit = Commit::new("xyz000", "chore(deps): upgrade lib", "BREAKING CHANGE: api changed");
        cl.parse_commit(&release_types(), &commit);

        // chore is not in release_types, but breaking change body triggers fallback parsing
        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].section_type, "chore");
        assert_eq!(cl.commits[0].scope, "deps");
        assert_eq!(cl.commits[0].desc, "upgrade lib");
        assert_eq!(cl.commits[0].breaking, "api changed");
    }

    #[test]
    fn parse_commit_ignores_unrecognized_type_without_breaking() {
        let mut cl = Changelog::new();
        let commit = Commit::new("aaa111", "docs: update readme", "");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 0);
    }

    #[test]
    fn parse_commit_special_chars_in_description() {
        let mut cl = Changelog::new();
        let commit = Commit::new("ccc333", "feat: load env vars from donder-release.env", "");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].desc, "load env vars from donder-release.env");
    }

    #[test]
    fn parse_commit_scope_with_slash() {
        let mut cl = Changelog::new();
        let commit = Commit::new("ddd444", "feat(ui/auth): add login page", "");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].scope, "ui/auth");
        assert_eq!(cl.commits[0].desc, "add login page");
    }

    #[test]
    fn parse_commit_scoped_with_dots_and_dashes() {
        let mut cl = Changelog::new();
        let commit = Commit::new("bbb222", "feat(my-lib.core): add parser", "");
        cl.parse_commit(&release_types(), &commit);

        assert_eq!(cl.commits.len(), 1);
        assert_eq!(cl.commits[0].scope, "my-lib.core");
    }

    // write_notes tests

    #[test]
    fn write_notes_first_release_no_compare_link() {
        let mut cl = Changelog::new();
        cl.next_release_version = "v1.0.0".to_string();
        cl.commits.push(ChangelogCommit {
            section_type: "feat".to_string(),
            scope: "".to_string(),
            desc: "initial feature".to_string(),
            breaking: "".to_string(),
            hash: "abc123".to_string(),
        });

        cl.write_notes(&"".to_string(), &default_types(), "https://github.com/owner/repo").unwrap();

        assert!(cl.notes.starts_with("## v1.0.0\r\n"));
        assert!(!cl.notes.contains("compare"));
        assert!(cl.notes.contains("### Features"));
        assert!(cl.notes.contains("initial feature"));
        assert!(cl.notes.contains("[abc123](https://github.com/owner/repo/commit/abc123)"));
    }

    #[test]
    fn write_notes_subsequent_release_has_compare_link() {
        let mut cl = Changelog::new();
        cl.next_release_version = "v1.1.0".to_string();
        cl.commits.push(ChangelogCommit {
            section_type: "feat".to_string(),
            scope: "".to_string(),
            desc: "new feature".to_string(),
            breaking: "".to_string(),
            hash: "def456".to_string(),
        });

        cl.write_notes(&"v1.0.0".to_string(), &default_types(), "https://github.com/owner/repo").unwrap();

        assert!(cl.notes.contains("[v1.1.0](https://github.com/owner/repo/compare/v1.0.0...v1.1.0)"));
    }

    #[test]
    fn write_notes_sections_ordered_by_release_types() {
        let mut cl = Changelog::new();
        cl.next_release_version = "v1.0.0".to_string();
        // Add fix first, then feat - output should order feat before fix
        cl.commits.push(ChangelogCommit {
            section_type: "fix".to_string(), scope: "".to_string(),
            desc: "a fix".to_string(), breaking: "".to_string(), hash: "aaa".to_string(),
        });
        cl.commits.push(ChangelogCommit {
            section_type: "feat".to_string(), scope: "".to_string(),
            desc: "a feature".to_string(), breaking: "".to_string(), hash: "bbb".to_string(),
        });

        cl.write_notes(&"".to_string(), &default_types(), "https://github.com/o/r").unwrap();

        let feat_pos = cl.notes.find("### Features").unwrap();
        let fix_pos = cl.notes.find("### Bug Fixes").unwrap();
        assert!(feat_pos < fix_pos, "Features section should come before Bug Fixes");
    }

    #[test]
    fn write_notes_scope_grouping() {
        let mut cl = Changelog::new();
        cl.next_release_version = "v1.0.0".to_string();
        cl.commits.push(ChangelogCommit {
            section_type: "fix".to_string(), scope: "parser".to_string(),
            desc: "fix parsing".to_string(), breaking: "".to_string(), hash: "c1".to_string(),
        });
        cl.commits.push(ChangelogCommit {
            section_type: "fix".to_string(), scope: "parser".to_string(),
            desc: "fix edge case".to_string(), breaking: "".to_string(), hash: "c2".to_string(),
        });

        cl.write_notes(&"".to_string(), &default_types(), "https://github.com/o/r").unwrap();

        assert!(cl.notes.contains("- **parser:**"));
        assert!(cl.notes.contains("  - fix parsing"));
        assert!(cl.notes.contains("  - fix edge case"));
    }

    #[test]
    fn write_notes_breaking_changes_section() {
        let mut cl = Changelog::new();
        cl.next_release_version = "v2.0.0".to_string();
        cl.commits.push(ChangelogCommit {
            section_type: "feat".to_string(), scope: "".to_string(),
            desc: "new api".to_string(), breaking: "old api removed".to_string(), hash: "brk1".to_string(),
        });

        cl.write_notes(&"v1.0.0".to_string(), &default_types(), "https://github.com/o/r").unwrap();

        assert!(cl.notes.contains("### BREAKING CHANGES"));
        assert!(cl.notes.contains("- old api removed"));
    }
}