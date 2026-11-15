use crate::github::PullRequestCommit;

const DEPENDABOT_LOGINS: [&str; 2] = ["dependabot[bot]", "dependabot"];

/// A Dependabot comment command the watchdog may post on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Rebase,
    Recreate,
}

impl Command {
    pub fn as_str(self) -> &'static str {
        match self {
            Command::Rebase => "@dependabot rebase",
            Command::Recreate => "@dependabot recreate",
        }
    }
}

/// Whether the given login is Dependabot's own bot account.
pub fn is_dependabot(login: &str) -> bool {
    DEPENDABOT_LOGINS
        .iter()
        .any(|known| login.eq_ignore_ascii_case(known))
}

/// Next command to ask for, given the command the watchdog already posted.
///
/// `@dependabot rebase` is the normal request. It refuses branches carrying
/// commits Dependabot did not author, so `@dependabot recreate` — the same
/// rebase with that check skipped — is the only escalation. Once recreate has
/// been asked for there is nothing left to try automatically.
pub fn next_command(previous: Option<Command>) -> Option<Command> {
    match previous {
        None => Some(Command::Rebase),
        Some(Command::Rebase) => Some(Command::Recreate),
        Some(Command::Recreate) => None,
    }
}

/// Latest command already posted by the watchdog, given its own comment bodies
/// in chronological order.
pub fn last_requested_command<'a>(
    comment_bodies: impl DoubleEndedIterator<Item = &'a str>,
) -> Option<Command> {
    comment_bodies.rev().find_map(command_in)
}

pub fn comment_body(command: Command) -> String {
    format!(
        "{}\n\nThis pull request conflicts with its base branch, so pr-watchdog asked \
         Dependabot to refresh it.",
        command.as_str()
    )
}

/// Whether the pull request branch holds commits Dependabot did not author.
///
/// `@dependabot recreate` force-pushes the branch, discarding those commits, so
/// such a pull request is left to a human instead.
pub fn has_foreign_commits(commits: &[PullRequestCommit]) -> bool {
    commits.iter().any(|commit| !is_dependabot_commit(commit))
}

fn command_in(body: &str) -> Option<Command> {
    if body.contains(Command::Recreate.as_str()) {
        Some(Command::Recreate)
    } else if body.contains(Command::Rebase.as_str()) {
        Some(Command::Rebase)
    } else {
        None
    }
}

fn is_dependabot_commit(commit: &PullRequestCommit) -> bool {
    let github_login = commit.author.as_ref().map(|author| author.login.as_str());
    github_login
        .or_else(|| {
            commit
                .commit
                .author
                .as_ref()
                .and_then(|author| author.name.as_deref())
        })
        .is_some_and(is_dependabot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{Commit, GitAuthor, User};

    fn commit(github_login: Option<&str>, git_author: Option<&str>) -> PullRequestCommit {
        PullRequestCommit {
            author: github_login.map(|login| User {
                login: login.to_string(),
            }),
            commit: Commit {
                author: Some(GitAuthor {
                    name: git_author.map(str::to_string),
                }),
            },
        }
    }

    #[test]
    fn is_dependabot_accepts_bot_login_and_bare_name() {
        assert!(is_dependabot("dependabot[bot]"));
        assert!(is_dependabot("dependabot"));
        assert!(is_dependabot("Dependabot[bot]"));
    }

    #[test]
    fn is_dependabot_rejects_other_logins() {
        assert!(!is_dependabot("dependabot-helper"));
        assert!(!is_dependabot("alice"));
        assert!(!is_dependabot(""));
    }

    #[test]
    fn next_command_starts_with_rebase() {
        assert_eq!(next_command(None), Some(Command::Rebase));
    }

    #[test]
    fn next_command_escalates_rebase_to_recreate() {
        assert_eq!(next_command(Some(Command::Rebase)), Some(Command::Recreate));
    }

    #[test]
    fn next_command_stops_after_recreate() {
        assert_eq!(next_command(Some(Command::Recreate)), None);
    }

    #[test]
    fn last_requested_command_reads_latest_comment() {
        let bodies = [
            "@dependabot rebase",
            "thanks!",
            "@dependabot recreate\n\nsome explanation",
        ];
        assert_eq!(
            last_requested_command(bodies.into_iter()),
            Some(Command::Recreate)
        );
    }

    #[test]
    fn last_requested_command_ignores_comments_without_commands() {
        let bodies = ["@dependabot rebase", "looks good to me"];
        assert_eq!(
            last_requested_command(bodies.into_iter()),
            Some(Command::Rebase)
        );
    }

    #[test]
    fn last_requested_command_none_without_commands() {
        assert_eq!(last_requested_command([].into_iter()), None);
    }

    #[test]
    fn comment_body_starts_with_the_command() {
        assert!(comment_body(Command::Rebase).starts_with("@dependabot rebase"));
        assert!(comment_body(Command::Recreate).starts_with("@dependabot recreate"));
    }

    #[test]
    fn comment_body_is_read_back_as_the_same_command() {
        for command in [Command::Rebase, Command::Recreate] {
            let body = comment_body(command);
            assert_eq!(
                last_requested_command([body.as_str()].into_iter()),
                Some(command)
            );
        }
    }

    #[test]
    fn has_foreign_commits_all_dependabot_commits() {
        let commits = [
            commit(Some("dependabot[bot]"), Some("dependabot[bot]")),
            commit(None, Some("dependabot[bot]")),
        ];
        assert!(!has_foreign_commits(&commits));
    }

    #[test]
    fn has_foreign_commits_detects_human_commit() {
        let commits = [
            commit(Some("dependabot[bot]"), Some("dependabot[bot]")),
            commit(Some("alice"), Some("Alice")),
        ];
        assert!(has_foreign_commits(&commits));
    }

    #[test]
    fn has_foreign_commits_falls_back_to_git_author_name() {
        let commits = [commit(None, Some("Alice"))];
        assert!(has_foreign_commits(&commits));
    }

    #[test]
    fn has_foreign_commits_treats_unknown_author_as_foreign() {
        let commits = [PullRequestCommit {
            author: None,
            commit: Commit { author: None },
        }];
        assert!(has_foreign_commits(&commits));
    }

    #[test]
    fn has_foreign_commits_empty_list() {
        assert!(!has_foreign_commits(&[]));
    }
}
