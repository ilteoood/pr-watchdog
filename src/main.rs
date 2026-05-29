mod config;
mod github;
mod watcher;

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use config::Config;
use github::GitHubClient;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env()?;
    let client = GitHubClient::new(&config.github_token)?;

    let me = client
        .authenticated_login()
        .await
        .context("failed to resolve authenticated GitHub user")?;
    info!(user = %me, "authenticated with GitHub");
    info!(
        repos = config.repos.len(),
        cron = %config.cron,
        "starting pr-watchdog"
    );

    // Run once immediately so the watchdog acts without waiting for the first tick.
    if let Err(err) = watcher::run_pass(&client, &config.repos, &me).await {
        error!(error = %err, "initial watchdog pass failed");
    }

    let scheduler = JobScheduler::new()
        .await
        .context("failed to create scheduler")?;

    let client = Arc::new(client);
    let repos = Arc::new(config.repos.clone());
    let me = Arc::new(me);

    let job = Job::new_async(config.cron.as_str(), move |_uuid, _lock| {
        let client = Arc::clone(&client);
        let repos = Arc::clone(&repos);
        let me = Arc::clone(&me);
        Box::pin(async move {
            info!("running scheduled watchdog pass");
            if let Err(err) = watcher::run_pass(&client, &repos, &me).await {
                error!(error = %err, "watchdog pass failed");
            }
        })
    })
    .with_context(|| format!("invalid CRON_PATTERN '{}'", config.cron))?;

    scheduler.add(job).await.context("failed to schedule job")?;
    scheduler.start().await.context("failed to start scheduler")?;

    info!("scheduler started; press Ctrl+C to stop");
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for shutdown signal")?;
    info!("shutting down");

    Ok(())
}
