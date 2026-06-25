use anyhow::Result;
use tracing::{debug, info, warn};

use crate::config::Repo;
use crate::github::{trusted_users_approved, GitHubClient};

/// Run a single watchdog pass over all configured repositories.
///
/// For every open pull request:
/// - if it was created by a trusted login (the authenticated user or anyone in
///   `TRUSTED_USERS`) and is ready to merge, merge it;
/// - if it was approved by a trusted login and is ready to merge, merge it;
/// - if it is behind its base branch, update it via the GitHub API (no clone).
pub async fn run_pass(
    client: &GitHubClient,
    repos: &[Repo],
    trusted_logins: &[String],
) -> Result<()> {
    for repo in repos {
        if let Err(err) = process_repo(client, repo, trusted_logins).await {
            warn!(repo = %repo, error = %err, "failed to process repository");
        }
    }
    Ok(())
}

async fn process_repo(client: &GitHubClient, repo: &Repo, trusted_logins: &[String]) -> Result<()> {
    let prs = client.list_open_pull_requests(repo).await?;
    info!(repo = %repo, count = prs.len(), "checking open pull requests");

    for pr in prs {
        if pr.draft {
            continue;
        }
        if let Err(err) =
            process_pull_request(client, repo, pr.number, &pr.title, trusted_logins).await
        {
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
    trusted_logins: &[String],
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
        debug!(repo = %repo, pr = number, "branch updated successfully");
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

    let created_by_trusted = trusted_logins.iter().any(|l| l == &detail.user.login);
    let approved_by_trusted = if created_by_trusted {
        false
    } else {
        let reviews = client.list_reviews(repo, number).await?;
        trusted_users_approved(&reviews, trusted_logins)
    };

    if created_by_trusted || approved_by_trusted {
        let reason = if created_by_trusted {
            "created by trusted user"
        } else {
            "approved by trusted user"
        };
        let author = detail.user.login.as_str();
        info!(
            repo = %repo,
            pr = number,
            %title,
            author,
            reason,
            "merging pull request"
        );
        client.merge_pull_request(repo, number).await?;
        debug!(repo = %repo, pr = number, "pull request merged successfully");
    }

    Ok(())
}
