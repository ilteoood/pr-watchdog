use anyhow::Result;
use tracing::{info, warn};

use crate::config::Repo;
use crate::github::{user_approved, GitHubClient};

/// Run a single watchdog pass over all configured repositories.
///
/// For every open pull request:
/// - if it is created by the authenticated user and ready to merge, merge it;
/// - if it is approved by the authenticated user and ready to merge, merge it;
/// - if it is behind its base branch, update it via the GitHub API (no clone).
pub async fn run_pass(client: &GitHubClient, repos: &[Repo], me: &str) -> Result<()> {
    for repo in repos {
        if let Err(err) = process_repo(client, repo, me).await {
            warn!(repo = %repo, error = %err, "failed to process repository");
        }
    }
    Ok(())
}

async fn process_repo(client: &GitHubClient, repo: &Repo, me: &str) -> Result<()> {
    let prs = client.list_open_pull_requests(repo).await?;
    info!(repo = %repo, count = prs.len(), "checking open pull requests");

    for pr in prs {
        if pr.draft {
            continue;
        }
        if let Err(err) = process_pull_request(client, repo, pr.number, &pr.title, me).await {
            warn!(repo = %repo, pr = pr.number, error = %err, "failed to process pull request");
        }
    }
    Ok(())
}

async fn process_pull_request(
    client: &GitHubClient,
    repo: &Repo,
    number: u64,
    title: &str,
    me: &str,
) -> Result<()> {
    let detail = client.get_pull_request(repo, number).await?;
    if detail.draft {
        return Ok(());
    }

    let state = detail.mergeable_state.as_str();

    // A PR that is behind its base branch needs to be updated first.
    if state == "behind" {
        info!(repo = %repo, pr = number, %title, "branch is behind base; updating via GitHub API");
        client.update_branch(repo, number).await?;
        return Ok(());
    }

    // Only attempt to merge when GitHub reports the branch as clean & mergeable.
    let ready = detail.mergeable == Some(true) && state == "clean";
    if !ready {
        info!(
            repo = %repo,
            pr = number,
            mergeable = ?detail.mergeable,
            state,
            "not ready to merge; skipping"
        );
        return Ok(());
    }

    let created_by_me = detail.user.login == me;
    let approved_by_me = if created_by_me {
        false
    } else {
        let reviews = client.list_reviews(repo, number).await?;
        user_approved(&reviews, me)
    };

    if created_by_me || approved_by_me {
        let reason = if created_by_me { "created by me" } else { "approved by me" };
        info!(repo = %repo, pr = number, %title, reason, "merging pull request");
        client.merge_pull_request(repo, number).await?;
    }

    Ok(())
}
