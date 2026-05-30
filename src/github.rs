use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::{header, Client, RequestBuilder, StatusCode};
use serde::Deserialize;
use tracing::warn;

use crate::config::Repo;

const API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("pr-watchdog/", env!("CARGO_PKG_VERSION"));

/// Default number of retries for transient failures and rate limiting.
const DEFAULT_MAX_RETRIES: u32 = 3;
/// Default base delay used for exponential backoff between retries.
const DEFAULT_BASE_DELAY: Duration = Duration::from_secs(1);
/// Upper bound on how long a single retry will ever wait.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

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
    base_url: String,
    merge_method: MergeMethod,
    max_retries: u32,
    base_delay: Duration,
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
        Self::with_base_url(API_BASE, token, merge_method)
    }

    /// Build a client targeting a custom API base URL. Primarily used in tests
    /// to point the client at a mock server.
    pub fn with_base_url(base_url: &str, token: &str, merge_method: MergeMethod) -> Result<Self> {
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

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            merge_method,
            max_retries: DEFAULT_MAX_RETRIES,
            base_delay: DEFAULT_BASE_DELAY,
        })
    }

    /// Override the retry behaviour (number of retries and backoff base delay).
    /// Mainly useful for keeping tests fast.
    pub fn with_retry_settings(mut self, max_retries: u32, base_delay: Duration) -> Self {
        self.max_retries = max_retries;
        self.base_delay = base_delay;
        self
    }

    /// Send a request, retrying on rate limiting and transient failures.
    ///
    /// `make` is called once per attempt so the request (including its body) can
    /// be rebuilt for each retry.
    async fn send_with_retry<F>(&self, make: F) -> Result<reqwest::Response>
    where
        F: Fn() -> RequestBuilder,
    {
        let mut attempt = 0u32;
        loop {
            match make().send().await {
                Ok(resp) => {
                    if attempt < self.max_retries && should_retry_status(&resp) {
                        let delay = retry_delay(&resp, attempt, self.base_delay);
                        warn!(
                            status = %resp.status(),
                            attempt = attempt + 1,
                            delay_secs = delay.as_secs_f64(),
                            "retrying GitHub API request after rate limit/server error"
                        );
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Ok(resp);
                }
                Err(err) => {
                    if attempt < self.max_retries && is_transient(&err) {
                        let delay = backoff(attempt, self.base_delay);
                        warn!(
                            error = %err,
                            attempt = attempt + 1,
                            delay_secs = delay.as_secs_f64(),
                            "retrying GitHub API request after transient error"
                        );
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(err.into());
                }
            }
        }
    }

    /// Return the login of the authenticated user.
    pub async fn authenticated_login(&self) -> Result<String> {
        let url = format!("{}/user", self.base_url);
        let resp = self.send_with_retry(|| self.client.get(&url)).await?;
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
                "{}/repos/{}/{}/pulls?state=open&per_page=100&page={page}",
                self.base_url, repo.owner, repo.name
            );
            let resp = self.send_with_retry(|| self.client.get(&url)).await?;
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
            "{}/repos/{}/{}/pulls/{number}",
            self.base_url, repo.owner, repo.name
        );
        let resp = self.send_with_retry(|| self.client.get(&url)).await?;
        let resp = ensure_success(resp).await?;
        resp.json()
            .await
            .context("failed to parse pull request detail")
    }

    /// List reviews for a pull request, following pagination.
    pub async fn list_reviews(&self, repo: &Repo, number: u64) -> Result<Vec<Review>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let url = format!(
                "{}/repos/{}/{}/pulls/{number}/reviews?per_page=100&page={page}",
                self.base_url, repo.owner, repo.name
            );
            let resp = self.send_with_retry(|| self.client.get(&url)).await?;
            let resp = ensure_success(resp).await?;
            let batch: Vec<Review> = resp.json().await.context("failed to parse reviews")?;
            let len = batch.len();
            all.extend(batch);
            if len < 100 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    /// Merge a pull request. Returns true on success.
    pub async fn merge_pull_request(&self, repo: &Repo, number: u64) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{number}/merge",
            self.base_url, repo.owner, repo.name
        );
        let merge_method = self.merge_method.as_str();
        let resp = self
            .send_with_retry(|| {
                self.client
                    .put(&url)
                    .json(&serde_json::json!({ "merge_method": merge_method }))
            })
            .await?;
        ensure_success(resp).await?;
        Ok(())
    }

    /// Update a pull request branch with the latest base branch using GitHub's
    /// update-branch API (no local clone required).
    pub async fn update_branch(&self, repo: &Repo, number: u64) -> Result<()> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{number}/update-branch",
            self.base_url, repo.owner, repo.name
        );
        let resp = self
            .send_with_retry(|| self.client.put(&url).header(header::CONTENT_LENGTH, "0"))
            .await?;
        ensure_success(resp).await?;
        Ok(())
    }
}

/// Whether a response should be retried (rate limiting or transient server error).
fn should_retry_status(resp: &reqwest::Response) -> bool {
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        return true;
    }
    // A 403 with no remaining rate-limit budget is GitHub's secondary rate limit.
    if status == StatusCode::FORBIDDEN {
        return resp
            .headers()
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.trim() == "0")
            .unwrap_or(false);
    }
    false
}

/// Whether a request error is transient and worth retrying.
fn is_transient(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Exponential backoff delay capped at [`MAX_RETRY_DELAY`].
fn backoff(attempt: u32, base_delay: Duration) -> Duration {
    let factor = 2u32.saturating_pow(attempt);
    base_delay.saturating_mul(factor).min(MAX_RETRY_DELAY)
}

/// Compute the delay before retrying a rate-limited/erroring response.
///
/// Honours `Retry-After` (seconds) when present, otherwise falls back to
/// exponential backoff. The result is always capped at [`MAX_RETRY_DELAY`].
fn retry_delay(resp: &reqwest::Response, attempt: u32, base_delay: Duration) -> Duration {
    if let Some(secs) = resp
        .headers()
        .get(header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        return Duration::from_secs(secs).min(MAX_RETRY_DELAY);
    }
    backoff(attempt, base_delay)
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

