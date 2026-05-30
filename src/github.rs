use anyhow::{anyhow, Context, Result};
use reqwest::{header, Client, StatusCode};
use serde::Deserialize;

use crate::config::Repo;

const API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("pr-watchdog/", env!("CARGO_PKG_VERSION"));

/// Merge method used when merging a pull request.
#[derive(Debug, Clone, Copy, Default)]
pub enum MergeMethod {
    #[default]
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

impl std::fmt::Display for MergeMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A thin GitHub REST API client tailored to the watchdog's needs.
#[derive(Clone)]
pub struct GitHubClient {
    client: Client,
    merge_method: MergeMethod,
}

/// Minimal representation of a pull request from the list endpoint.
#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub draft: bool,
}

/// Detailed pull request info including merge state.
#[derive(Debug, Deserialize)]
pub struct PullRequestDetail {
    #[serde(default)]
    pub draft: bool,
    /// Whether GitHub considers the PR mergeable. `None` while still computing.
    pub mergeable: Option<bool>,
    /// e.g. "clean", "behind", "blocked", "dirty", "unstable", "unknown".
    #[serde(default)]
    pub mergeable_state: String,
    pub user: User,
}

#[derive(Debug, Deserialize)]
pub struct User {
    pub login: String,
}

/// A pull request review.
#[derive(Debug, Deserialize)]
pub struct Review {
    pub user: Option<User>,
    /// "APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED", "PENDING".
    pub state: String,
}

impl GitHubClient {
    pub fn new(token: &str, merge_method: MergeMethod) -> Result<Self> {
        let mut headers = header::HeaderMap::new();
        let mut auth = header::HeaderValue::from_str(&format!("Bearer {token}"))
            .context("invalid GITHUB_TOKEN value")?;
        auth.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, auth);
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            header::HeaderValue::from_static("2022-11-28"),
        );

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self { client, merge_method })
    }

    /// Return the login of the authenticated user.
    pub async fn authenticated_login(&self) -> Result<String> {
        let url = format!("{API_BASE}/user");
        let resp = self.client.get(&url).send().await?;
        let resp = ensure_success(resp).await?;
        let user: User = resp
            .json()
            .await
            .context("failed to parse /user response")?;
        Ok(user.login)
    }

    /// List all open pull requests for a repository, following pagination.
    pub async fn list_open_pull_requests(&self, repo: &Repo) -> Result<Vec<PullRequest>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "{API_BASE}/repos/{}/{}/pulls?state=open&per_page=100&page={page}",
                repo.owner, repo.name
            );
            let resp = self.client.get(&url).send().await?;
            let resp = ensure_success(resp).await?;
            let batch: Vec<PullRequest> = resp
                .json()
                .await
                .context("failed to parse pull request list")?;
            let len = batch.len();
            all.extend(batch);
            if len < 100 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// Fetch detailed information for a single pull request.
    pub async fn get_pull_request(&self, repo: &Repo, number: u64) -> Result<PullRequestDetail> {
        let url = format!(
            "{API_BASE}/repos/{}/{}/pulls/{number}",
            repo.owner, repo.name
        );
        let resp = self.client.get(&url).send().await?;
        let resp = ensure_success(resp).await?;
        resp.json()
            .await
            .context("failed to parse pull request detail")
    }

    /// List reviews for a pull request.
    pub async fn list_reviews(&self, repo: &Repo, number: u64) -> Result<Vec<Review>> {
        let url = format!(
            "{API_BASE}/repos/{}/{}/pulls/{number}/reviews?per_page=100",
            repo.owner, repo.name
        );
        let resp = self.client.get(&url).send().await?;
        let resp = ensure_success(resp).await?;
        resp.json().await.context("failed to parse reviews")
    }

    /// Merge a pull request. Returns true on success.
    pub async fn merge_pull_request(&self, repo: &Repo, number: u64) -> Result<()> {
        let url = format!(
            "{API_BASE}/repos/{}/{}/pulls/{number}/merge",
            repo.owner, repo.name
        );
        let resp = self
            .client
            .put(&url)
            .json(&serde_json::json!({ "merge_method": self.merge_method.as_str() }))
            .send()
            .await?;
        ensure_success(resp).await?;
        Ok(())
    }

    /// Update a pull request branch with the latest base branch using GitHub's
    /// update-branch API (no local clone required).
    pub async fn update_branch(&self, repo: &Repo, number: u64) -> Result<()> {
        let url = format!(
            "{API_BASE}/repos/{}/{}/pulls/{number}/update-branch",
            repo.owner, repo.name
        );
        let resp = self
            .client
            .put(&url)
            .header(header::CONTENT_LENGTH, "0")
            .send()
            .await?;
        ensure_success(resp).await?;
        Ok(())
    }
}

/// Determine whether the authenticated user approved the pull request.
pub fn user_approved(reviews:&[Review], login: &str) -> bool {
    // The latest review state per user wins; iterate in order and track.
    let mut approved = false;
    for review in reviews {
        if review.user.as_ref().map(|u| u.login.as_str()) == Some(login) {
            match review.state.as_str() {
                "APPROVED" => approved = true,
                "CHANGES_REQUESTED" | "DISMISSED" => approved = false,
                _ => {}
            }
        }
    }
    approved
}

/// Convert an unsuccessful response into a descriptive error.
async fn ensure_success(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let url = resp.url().to_string();
    let body = resp.text().await.unwrap_or_default();
    let message = match status {
        StatusCode::UNAUTHORIZED => "authentication failed (check GITHUB_TOKEN)".to_string(),
        StatusCode::FORBIDDEN => format!("forbidden (rate limit or insufficient scope): {body}"),
        StatusCode::NOT_FOUND => {
            "resource not found (check repository name/permissions)".to_string()
        }
        _ => body,
    };
    Err(anyhow!(
        "GitHub API request to {url} failed with {status}: {message}"
    ))
}

