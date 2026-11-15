use anyhow::Result;
use tracing::{debug, info, warn};

use crate::config::Repo;
use crate::dependabot;
use crate::github::{trusted_users_approved, GitHubClient, PullRequest};

/// Run a single watchdog pass over all configured repositories.
///
/// For every open pull request that is either created by a trusted login
/// (the authenticated user or anyone in `TRUSTED_USERS`) or approved by a
/// trusted login:
/// - if it is behind its base branch, update it via the GitHub API (no clone);
/// - if it is ready to merge, merge it.
///
/// Conflicting Dependabot pull requests are handled separately, whatever their
/// author's trust: Dependabot is asked to resolve the conflict itself.
pub async fn run_pass(
    client: &GitHubClient,
    repos: &[Repo],
    me: &str,
    trusted_logins: &[String],
) -> Result<()> {
    for repo in repos {
        if let Err(err) = process_repo(client, repo, me, trusted_logins).await {
            warn!(repo = %repo, error = %err, "failed to process repository");
        }
    }
    Ok(())
}

async fn process_repo(
    client: &GitHubClient,
    repo: &Repo,
    me: &str,
    trusted_logins: &[String],
) -> Result<()> {
    let prs = client.list_open_pull_requests(repo).await?;
    info!(repo = %repo, count = prs.len(), "checking open pull requests");

    for pr in prs {
        if pr.draft {
            continue;
        }
        if let Err(err) = process_pull_request(client, repo, &pr, me, trusted_logins).await {
            warn!(repo = %repo, pr = pr.number, error = %err, "failed to process pull request");
        }
    }
    Ok(())
}

async fn process_pull_request(
    client: &GitHubClient,
    repo: &Repo,
    pr: &PullRequest,
    me: &str,
    trusted_logins: &[String],
) -> Result<()> {
    let number = pr.number;
    let detail = client.get_pull_request(repo, number).await?;
    if detail.draft {
        return Ok(());
    }

    if dependabot::is_dependabot(&detail.user.login) && detail.mergeable_state == "dirty" {
        return resolve_dependabot_conflict(client, repo, number, me).await;
    }

    let created_by_trusted = trusted_logins.iter().any(|l| l == &detail.user.login);
    let approved_by_trusted = if created_by_trusted {
        false
    } else {
        let reviews = client.list_reviews(repo, number).await?;
        trusted_users_approved(&reviews, trusted_logins)
    };

    if !created_by_trusted && !approved_by_trusted {
        info!(
            repo = %repo,
            pr = number,
            title = %pr.title,
            author = detail.user.login.as_str(),
            "pull request is not trusted; skipping"
        );
        return Ok(());
    }

    let state = detail.mergeable_state.as_str();

    // A PR that is behind its base branch needs to be updated first.
    if state == "behind" {
        info!(
            repo = %repo,
            pr = number,
            title = %pr.title,
            "branch is behind base; updating via GitHub API"
        );
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

    let reason = if created_by_trusted {
        "created by trusted user"
    } else {
        "approved by trusted user"
    };
    let author = detail.user.login.as_str();
    info!(
        repo = %repo,
        pr = number,
        title = %pr.title,
        author,
        reason,
        "merging pull request"
    );
    client.merge_pull_request(repo, number).await?;
    debug!(repo = %repo, pr = number, "pull request merged successfully");

    Ok(())
}

/// Ask Dependabot to resolve a conflicting pull request: a rebase first, a
/// forced rebase next, then a human.
async fn resolve_dependabot_conflict(
    client: &GitHubClient,
    repo: &Repo,
    number: u64,
    me: &str,
) -> Result<()> {
    let commits = client.list_pull_request_commits(repo, number).await?;
    if dependabot::has_foreign_commits(&commits) {
        warn!(
            repo = %repo,
            pr = number,
            "dependabot pull request conflicts and carries commits dependabot did not \
             author; leaving it to a human, since a forced rebase would drop them"
        );
        return Ok(());
    }

    let comments = client.list_issue_comments(repo, number).await?;
    let previous = dependabot::last_requested_command(
        comments
            .iter()
            .filter(|comment| comment.user.login == me)
            .map(|comment| comment.body.as_str()),
    );

    let Some(command) = dependabot::next_command(previous) else {
        warn!(
            repo = %repo,
            pr = number,
            "dependabot pull request still conflicts after a forced rebase; leaving it to a human"
        );
        return Ok(());
    };

    client
        .create_issue_comment(repo, number, &dependabot::comment_body(command))
        .await?;
    info!(
        repo = %repo,
        pr = number,
        command = command.as_str(),
        "asked dependabot to resolve a merge conflict"
    );
    Ok(())
}
