use anyhow::{anyhow, Result};
use chrono_tz::Tz;
use clap::Parser;
use std::collections::HashSet;
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

    /// IANA timezone the cron schedule is evaluated in (e.g. `UTC`, `Europe/Rome`).
    #[arg(long, env = "TZ", default_value = "UTC", value_parser = parse_tz)]
    pub tz: Tz,

    /// Additional GitHub logins (beyond the authenticated user) whose authored
    /// or approved pull requests are also eligible to be auto-merged.
    /// Comma, space, or newline separated.
    #[arg(long, env = "TRUSTED_USERS", value_parser = parse_trusted_users, default_value = "")]
    pub trusted_users: TrustedUserList,
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

/// A deduplicated list of GitHub logins trusted alongside the authenticated user.
#[derive(Debug, Clone, Default)]
pub struct TrustedUserList(pub Vec<String>);

impl std::ops::Deref for TrustedUserList {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.0
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

fn parse_tz(raw: &str) -> Result<Tz, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("TZ must not be empty".to_string());
    }
    Tz::from_str(trimmed).map_err(|_| {
        format!("invalid TZ '{raw}', expected an IANA name like 'UTC' or 'Europe/Rome'")
    })
}

/// Parse a list of GitHub logins separated by commas, whitespace, or newlines.
/// Duplicates are removed (case-insensitively) while preserving the first
/// occurrence's original case.
fn parse_trusted_users(raw: &str) -> Result<TrustedUserList, String> {
    let mut logins = Vec::new();
    let mut seen = HashSet::new();
    for entry in raw.split([',', '\n', ' ', '\t']) {
        let login = entry.trim();
        if login.is_empty() {
            continue;
        }
        if seen.insert(login.to_ascii_lowercase()) {
            logins.push(login.to_string());
        }
    }
    Ok(TrustedUserList(logins))
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

    #[test]
    fn parse_tz_accepts_iana_names() {
        assert!(matches!(parse_tz("UTC").unwrap(), Tz::UTC));
        assert!(matches!(parse_tz("Europe/Rome").unwrap(), Tz::Europe__Rome));
        assert!(matches!(
            parse_tz("America/New_York").unwrap(),
            Tz::America__New_York
        ));
    }

    #[test]
    fn parse_tz_accepts_iana_names_with_surrounding_whitespace() {
        assert!(matches!(
            parse_tz("  Europe/Rome  ").unwrap(),
            Tz::Europe__Rome
        ));
    }

    #[test]
    fn parse_tz_rejects_empty() {
        assert!(parse_tz("").is_err());
        assert!(parse_tz("   ").is_err());
    }

    #[test]
    fn parse_tz_rejects_unknown() {
        assert!(parse_tz("Atlantis").is_err());
        assert!(parse_tz("Europe/Atlantis").is_err());
    }

    #[test]
    fn parse_trusted_users_empty_is_ok() {
        let list = parse_trusted_users("").unwrap();
        assert!(list.0.is_empty());
    }

    #[test]
    fn parse_trusted_users_whitespace_only_is_ok() {
        let list = parse_trusted_users("   ").unwrap();
        assert!(list.0.is_empty());
    }

    #[test]
    fn parse_trusted_users_single() {
        let list = parse_trusted_users("alice").unwrap();
        assert_eq!(list.0, vec!["alice".to_string()]);
    }

    #[test]
    fn parse_trusted_users_mixed_separators() {
        let list = parse_trusted_users("alice, bob\ncarol\tdave eve").unwrap();
        assert_eq!(
            list.0,
            vec![
                "alice".to_string(),
                "bob".to_string(),
                "carol".to_string(),
                "dave".to_string(),
                "eve".to_string()
            ]
        );
    }

    #[test]
    fn parse_trusted_users_trims_whitespace_around_entries() {
        let list = parse_trusted_users("  alice  ,  bob  ").unwrap();
        assert_eq!(list.0, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn parse_trusted_users_skips_empty_entries() {
        let list = parse_trusted_users(",,alice,, ,bob,").unwrap();
        assert_eq!(list.0, vec!["alice".to_string(), "bob".to_string()]);
    }

    #[test]
    fn parse_trusted_users_dedupes_preserving_first_case() {
        let list = parse_trusted_users("alice, Alice, ALICE, bob").unwrap();
        assert_eq!(list.0, vec!["alice".to_string(), "bob".to_string()]);
    }
}
