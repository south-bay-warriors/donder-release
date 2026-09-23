use std::time::Duration;
use anyhow::{Result, anyhow, bail};
use chrono::Local;
use reqwest::{StatusCode, header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT}};
use serde::{Serialize, Deserialize};
use crate::git::parse_version;

/// Attempts at creating a GitHub release before giving up
const PUBLISH_ATTEMPTS: u64 = 3;
const REQUEST_TIMEOUT_SECS: u64 = 30;

#[derive(Default, Debug)]
pub struct GithubApi {
    /// The path to the git repository
    pub api_url: String,

    // to be used in request headers
    content_type: String,
    user_agent: String,
    authorization: String,
}

#[derive(Deserialize)]
pub struct Release {
    pub id: u64,
    pub tag_name: String,
    pub prerelease: bool,
}

impl GithubApi {
    pub fn new(token: &str, owner: &str, repo: &str) -> Self {
        Self {
            api_url: format!("https://api.github.com/repos/{}/{}", owner, repo),
            content_type: "application/vnd.github+json".to_string(),
            user_agent: "donder-release".to_string(),
            authorization: format!("Bearer {}", token)
        }
    }

    pub async fn publish_release(&self, release_tag: &str, tag_prefix: &str, release_notes: &str) -> Result<()> {
        let version = release_tag.replace(tag_prefix, "");
        let request_body = PostRelease {
            tag_name: release_tag.to_string(),
            name: release_tag.to_string(),
            body: release_notes.to_string(),
            prerelease: parse_version(&version).is_some_and(|v| !v.pre.is_empty()),
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()?;

        let mut last_error = None;

        for attempt in 1..=PUBLISH_ATTEMPTS {
            if attempt > 1 {
                tokio::time::sleep(Duration::from_secs(2 * (attempt - 1))).await;

                // A failed attempt may still have created the release, so don't create it twice
                match self.release_exists(&client, release_tag).await {
                    Result::Ok(true) => return Ok(()),
                    Result::Ok(false) => {},
                    Err(e) => {
                        last_error = Some(e);
                        continue;
                    }
                }
            }

            match self.create_release(&client, &request_body).await {
                Result::Ok(()) => return Ok(()),
                Err(e) => {
                    if attempt < PUBLISH_ATTEMPTS {
                        logInfo!("Creating GitHub release failed, retrying ({}/{}): {}", attempt, PUBLISH_ATTEMPTS, e);
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("failed to create GitHub release")))
    }

    async fn create_release(&self, client: &reqwest::Client, request_body: &PostRelease) -> Result<()> {
        let response = client
            .post(format!("{}/releases", &self.api_url))
            .header(CONTENT_TYPE, &self.content_type)
            .header(USER_AGENT, &self.user_agent)
            .header(AUTHORIZATION, &self.authorization)
            .json(request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            // get error message from response
            let error_message = response.text().await?;
            bail!(error_message);
        }

        Ok(())
    }

    /// URL of the release for a tag, with the tag encoded as a single path segment
    /// since tag names can contain reserved characters such as `/` and `#`
    fn release_by_tag_url(&self, release_tag: &str) -> Result<reqwest::Url> {
        let mut url = reqwest::Url::parse(&self.api_url)?;
        url.path_segments_mut()
            .map_err(|_| anyhow!("invalid api url: {}", self.api_url))?
            .extend(["releases", "tags", release_tag]);
        Ok(url)
    }

    async fn release_exists(&self, client: &reqwest::Client, release_tag: &str) -> Result<bool> {
        let response = client
            .get(self.release_by_tag_url(release_tag)?)
            .header(CONTENT_TYPE, &self.content_type)
            .header(USER_AGENT, &self.user_agent)
            .header(AUTHORIZATION, &self.authorization)
            .send()
            .await?;

        match response.status() {
            StatusCode::OK => Ok(true),
            StatusCode::NOT_FOUND => Ok(false),
            status => bail!("failed to check for release {} ({}): {}", release_tag, status, response.text().await?),
        }
    }

    pub async fn clean_pre_releases(&self, tag_prefix: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .get(format!("{}/releases", &self.api_url))
            .header(CONTENT_TYPE, &self.content_type)
            .header(USER_AGENT, &self.user_agent)
            .header(AUTHORIZATION, &self.authorization)
            .send()
            .await?;

        if !response.status().is_success() {
            // get error message from response
            let error_message = response.text().await?;
            println!("error: {}", error_message);
            bail!(error_message);
        }

        let releases: Vec<Release> = response.json().await?;
        let pre_releases: Vec<&Release> = releases.iter().filter(|r| r.prerelease).collect();

        for release in pre_releases {
            let tag = release.tag_name.replace(tag_prefix, "");
            let Some(version) = parse_version(&tag) else {
                continue;
            };
            if !version.pre.is_empty() {
                let response = client
                    .delete(format!("{}/releases/{}", &self.api_url, release.id))
                    .header(CONTENT_TYPE, &self.content_type)
                    .header(USER_AGENT, &self.user_agent)
                    .header(AUTHORIZATION, &self.authorization)
                    .send()
                    .await?;

                if !response.status().is_success() {
                    // get error message from response
                    let error_message = response.text().await?;
                    println!("error: {}", error_message);
                    bail!(error_message);
                }
            }
        }

        Ok(())
    }
}

#[derive(Serialize)]
struct PostRelease {
    tag_name: String,
    name: String,
    body: String,
    prerelease: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_by_tag_url_plain_tag() {
        let api = GithubApi::new("token", "owner", "repo");
        assert_eq!(
            api.release_by_tag_url("v1.2.10").unwrap().as_str(),
            "https://api.github.com/repos/owner/repo/releases/tags/v1.2.10",
        );
    }

    #[test]
    fn release_by_tag_url_package_tag() {
        let api = GithubApi::new("token", "owner", "repo");
        assert_eq!(
            api.release_by_tag_url("my-pkg@v1.0.0").unwrap().as_str(),
            "https://api.github.com/repos/owner/repo/releases/tags/my-pkg@v1.0.0",
        );
    }

    #[test]
    fn release_by_tag_url_encodes_reserved_characters() {
        let api = GithubApi::new("token", "owner", "repo");
        assert_eq!(
            api.release_by_tag_url("release/v1#2?x").unwrap().as_str(),
            "https://api.github.com/repos/owner/repo/releases/tags/release%2Fv1%232%3Fx",
        );
    }
}
