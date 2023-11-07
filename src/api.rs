use anyhow::{Result, bail};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Serialize, Deserialize};
use semver::Version;

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
            prerelease: !Version::parse(&version).unwrap().pre.is_empty(),
        };

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/releases", &self.api_url))
            .header(CONTENT_TYPE, &self.content_type)
            .header(USER_AGENT, &self.user_agent)
            .header(AUTHORIZATION, &self.authorization)
            .json(&request_body)
            .send()
            .await?;

        if !response.status().is_success() {
            // get error message from response
            let error_message = response.text().await?;
            println!("error: {}", error_message);
            bail!(error_message);
        }

        Ok(())
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
            let version = Version::parse(&tag).unwrap();
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
