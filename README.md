# pr-watchdog

A small Rust service that watches GitHub repositories and, on a cron schedule:

- **Merges** open pull requests **created by you** that are ready to merge.
- **Merges** open pull requests **approved by you** that are ready to merge.
- **Merges** open pull requests **created or approved by any login listed in
  `TRUSTED_USERS`** that are ready to merge. This lets the watchdog act on
  behalf of multiple trusted users without sharing credentials.
- **Updates** pull requests that are behind their base branch using GitHub's
  [update-branch API](https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request-branch)
  (no local clone is ever performed). This merges the latest base branch into the
  PR branch; it is not a history-rewriting `git rebase`, regardless of the
  configured `MERGE_METHOD`.
- **Unblocks** Dependabot pull requests that conflict with their base branch
  by commenting `@dependabot rebase`, escalating to `@dependabot recreate` when a
  rebase was not enough.

## Configuration

Everything is configured through environment variables:

| Variable        | Required | Description                                                                  |
| --------------- | -------- | ---------------------------------------------------------------------------- |
| `GITHUB_TOKEN`  | yes      | Token used to authenticate against the GitHub API (needs repo write access). |
| `WATCHED_REPOS` | yes      | Comma/space/newline separated list of `owner/repo` to watch.                 |
| `CRON_PATTERN`  | no       | 7-field cron expression. Defaults to `0 */5 8-18 * * Mon-Fri *`.             |
| `MERGE_METHOD`  | no       | Merge strategy: `merge`, `squash`, or `rebase`. Defaults to `merge`.         |
| `TZ`            | no       | IANA timezone the cron schedule is evaluated in. Defaults to `UTC`.          |
| `TRUSTED_USERS` | no       | Comma/space/newline separated GitHub logins whose authored or approved PRs are also auto-merged. Defaults to empty. |
| `RUST_LOG`      | no       | Log verbosity (`info` by default).                                           |

The cron expression has 7 fields: `sec min hour day month day-of-week year`.
The default runs every 5 minutes, between 8am and 6pm, Monday to Friday,
evaluated in the timezone given by `TZ` (default `UTC`).

See [.env.example](.env.example) for a template.

### GitHub token permissions

The token in `GITHUB_TOKEN` must be able to read and write pull requests on every
watched repository:

- **Fine-grained personal access token** (recommended): grant the repositories
  you want to watch access to these repository permissions:
  - `Pull requests`: **Read and write** (list and merge PRs, update
    branches, and comment on Dependabot PRs).
  - `Contents`: **Read and write** (required by the update-branch API).
  - `Metadata`: **Read-only** (mandatory for all fine-grained tokens).
- **Classic personal access token**: the `repo` scope covers all of the above
  (use `public_repo` if you only watch public repositories).

The token must belong to the user whose authored/approved pull requests should be
merged, since the watchdog resolves "you" from the authenticated user. To merge PRs
created or approved by **additional** users without giving them commit access,
list their GitHub logins in `TRUSTED_USERS`. Merging still happens through the
authenticated user's token, so reviewers should still trust the token holder.

`TRUSTED_USERS` is parsed like `WATCHED_REPOS`: comma, whitespace, or newline
separated, with surrounding whitespace trimmed and case-insensitive duplicates
collapsed. Logins are matched against the PR author and reviewers as GitHub
returns them — use the canonical lowercase login to be safe.

## Running

```sh
export GITHUB_TOKEN=ghp_xxx
export WATCHED_REPOS="ilteoood/pr-watchdog"
export CRON_PATTERN="0 */5 8-18 * * Mon-Fri *"

cargo run --release
```

The watchdog performs one pass immediately at startup, then repeats according to
the cron schedule until interrupted (Ctrl+C).

## How "ready to merge" is decided

The service reads each PR's `mergeable` flag and `mergeable_state` from GitHub:

- `mergeable_state == "behind"` → the branch is updated with its base branch.
- `mergeable == true` and `mergeable_state == "clean"` → the PR is eligible to merge,
  and is merged if it was created or approved by you or any login in `TRUSTED_USERS`.
- Any other state (e.g. `blocked`, `dirty`, `unstable`, `unknown`) is skipped.

Draft pull requests are always skipped.

## Conflicting Dependabot pull requests

Dependabot pull requests are handled whatever their author's trust: when GitHub
reports one as conflicting (`mergeable_state == "dirty"`), the watchdog asks
Dependabot to refresh the branch by commenting on the pull request. It never
pushes to a Dependabot branch itself, so Dependabot keeps ownership of its
updates.

The escalation follows what Dependabot actually supports:

1. `@dependabot rebase` — the normal request. It rebases the pull request on its
   base branch, keeping any commit already on the branch.
2. `@dependabot recreate` — only after a rebase. This is a forced rebase: it
   discards the branch and rebuilds it, so it also drops commits added by hand.
3. Nothing else — once a recreate has been requested and the pull request still
   conflicts, the watchdog logs it and leaves the pull request to a human.

A pull request that already carries commits Dependabot did not author is never
sent a recreate, since that would throw those commits away; it is left to a
human from the start. The escalation state is read back from the watchdog's own
comments on the pull request, so it survives restarts and needs no storage.

## Docker Compose

```yaml
services:
  pr-watchdog:
    image: ilteoood/pr-watchdog:latest
    restart: unless-stopped
    environment:
      GITHUB_TOKEN: ghp_xxx
      WATCHED_REPOS: "owner/repo1,owner/repo2"
      CRON_PATTERN: "0 */5 8-18 * * Mon-Fri *"
      TZ: Europe/Rome
      MERGE_METHOD: merge
      TRUSTED_USERS: "trusted-colleague,another-dev"
      RUST_LOG: info
```
