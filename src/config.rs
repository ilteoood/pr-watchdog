use anyhow::{anyhow, Result};
use clap::Parser;
use std::str::FromStr;

use crate::github::MergeMethod;

/// Runtime configuration sourced entirely from environment variables via clap.
#[derive(Debug, Clone, Parser)]
#[command(author, version, about)]
pub struct Config {
    /// GitHub token used to authenticate API calls.
    #[arg(long, env = "GITHUB_TOKEN", hide_env_values = true)]
    pub github_token: String,

    /// Repositories to watch as `owner/repo`, separated by comma, space, or newline.
    #[arg(long, env = "WATCHED_REPOS", value_parser = parse_repos)]
    pub repos: RepoList,

    /// Cron expression controlling how often the watchdog runs.
    /// 7 fields: `sec min hour day month day-of-week year`.
    #[arg(long, env = "CRON_PATTERN", default_value = "0 */5 8-18 * * Mon-Fri *")]
    pub cron: String,

    /// Merge method to use: `merge`, `squash`, or `rebase`.
    #[arg(long, env = "MERGE_METHOD", default_value = "merge", value_parser = parse_merge_method)]
    pub merge_method: MergeMethod,
}

impl Config {
    /// Build configuration from environment variables (and CLI arguments).
    pub fn from_env() -> Result<Self> {
        let config = Config::try_parse()?;
        if config.github_token.trim().is_empty() {
            return Err(anyhow!("GITHUB_TOKEN must not be empty"));
        }
        if config.repos.0.is_empty() {
            return Err(anyhow!(
                "WATCHED_REPOS did not contain any valid repositories"
            ));
        }
        Ok(config)
    }
}

/// A parsed, non-empty list of repositories.
#[derive(Debug, Clone)]
pub struct RepoList(pub Vec<Repo>);

impl std::ops::Deref for RepoList {
    type Target = [Repo];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A single GitHub repository to watch.
#[derive(Debug, Clone)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl std::fmt::Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// Parse a list of `owner/repo` entries separated by commas, whitespace, or newlines.
fn parse_repos(raw: &str) -> Result<RepoList, String> {
    let mut repos = Vec::new();
    for entry in raw.split([',', '\n', ' ', '\t']) {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (owner, name) = entry
            .split_once('/')
            .ok_or_else(|| format!("invalid repository '{entry}', expected 'owner/repo'"))?;
        let owner = owner.trim();
        let name = name.trim();
        if owner.is_empty() || name.is_empty() {
            return Err(format!(
                "invalid repository '{entry}', expected 'owner/repo'"
            ));
        }
        repos.push(Repo {
            owner: owner.to_string(),
            name: name.to_string(),
        });
    }
    if repos.is_empty() {
        return Err("no valid repositories provided".to_string());
    }
    Ok(RepoList(repos))
}

fn parse_merge_method(raw: &str) -> Result<MergeMethod, String> {
    MergeMethod::from_str(raw)
}

impl FromStr for MergeMethod {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "merge" => Ok(MergeMethod::Merge),
            "squash" => Ok(MergeMethod::Squash),
            "rebase" => Ok(MergeMethod::Rebase),
            _ => Err(format!(
                "invalid merge method '{s}', expected 'merge', 'squash', or 'rebase'"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &RepoList) -> Vec<String> {
        list.0.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn parse_repos_single() {
        let list = parse_repos("owner/repo").unwrap();
        assert_eq!(names(&list), vec!["owner/repo"]);
    }

    #[test]
    fn parse_repos_mixed_separators() {
        let list = parse_repos("a/b, c/d\ne/f\tg/h i/j").unwrap();
        assert_eq!(names(&list), vec!["a/b", "c/d", "e/f", "g/h", "i/j"]);
    }

    #[test]
    fn parse_repos_trims_whitespace_around_parts() {
        let list = parse_repos("  owner/repo  ").unwrap();
        assert_eq!(names(&list), vec!["owner/repo"]);
    }

    #[test]
    fn parse_repos_skips_empty_entries() {
        let list = parse_repos(",,a/b,, ,c/d,").unwrap();
        assert_eq!(names(&list), vec!["a/b", "c/d"]);
    }

    #[test]
    fn parse_repos_rejects_missing_slash() {
        assert!(parse_repos("ownerrepo").is_err());
    }

    #[test]
    fn parse_repos_rejects_empty_owner_or_name() {
        assert!(parse_repos("/repo").is_err());
        assert!(parse_repos("owner/").is_err());
    }

    #[test]
    fn parse_repos_rejects_empty_input() {
        assert!(parse_repos("   ").is_err());
        assert!(parse_repos("").is_err());
    }

    #[test]
    fn parse_merge_method_accepts_known_values() {
        assert!(matches!(
            parse_merge_method("merge").unwrap(),
            MergeMethod::Merge
        ));
        assert!(matches!(
            parse_merge_method("SQUASH").unwrap(),
            MergeMethod::Squash
        ));
        assert!(matches!(
            parse_merge_method("  Rebase ").unwrap(),
            MergeMethod::Rebase
        ));
    }

    #[test]
    fn parse_merge_method_rejects_unknown() {
        assert!(parse_merge_method("fast-forward").is_err());
    }
}
