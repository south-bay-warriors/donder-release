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
        let pattern = r"^(TOKENS){1}(\([\w\-\.]+\))?(!)?: ([\w ]+)";
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
        for line in git_commit.subject.lines() {
            // Breaking changes
            if line.starts_with("BREAKING CHANGE: ") {
                commit.breaking = line.replace("BREAKING CHANGE: ", "");
                // Get commit info if no section type is found, this can happen if the commit
                // is not in the range of release_types but it's still relevant for the changelog
                // because it contains a breaking change, which should trigger a major release.
                if commit.section_type.is_empty() {
                    let re = Regex::new(r"^(\w+)(\([\w\-\.]+\))?(!)?: ([\w ]+)").unwrap();
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

        Ok(())
    }
}