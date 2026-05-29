# pr-watchdog

A small Rust service that watches GitHub repositories and, on a cron schedule:

- **Merges** open pull requests **created by you** that are ready to merge.
- **Merges** open pull requests **approved by you** that are ready to merge.
- **Rebases / updates** pull requests that are behind their base branch using
  GitHub's [update-branch API](https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request-branch)
  (no local clone is ever performed).

## Configuration

Everything is configured through environment variables:

| Variable        | Required | Description                                                                  |
| --------------- | -------- | ---------------------------------------------------------------------------- |
| `GITHUB_TOKEN`  | yes      | Token used to authenticate against the GitHub API (needs repo write access). |
| `WATCHED_REPOS` | yes      | Comma/space/newline separated list of `owner/repo` to watch.                 |
| `CRON_PATTERN`  | no       | 7-field cron expression. Defaults to `0 */5 8-18 * * Mon-Fri *`.             |
| `RUST_LOG`      | no       | Log verbosity (`info` by default).                                           |

The cron expression has 7 fields: `sec min hour day month day-of-week year`.
The default runs every 5 minutes, between 8am and 6pm, Monday to Friday.

See [.env.example](.env.example) for a template.

## Running

```sh
export GITHUB_TOKEN=ghp_xxx
export WATCHED_REPOS="HSEIreland/hse-health-app-webportal"
export CRON_PATTERN="0 */5 8-18 * * Mon-Fri *"

cargo run --release
```

The watchdog performs one pass immediately at startup, then repeats according to
the cron schedule until interrupted (Ctrl+C).

## How "ready to merge" is decided

The service reads each PR's `mergeable` flag and `mergeable_state` from GitHub:

- `mergeable_state == "behind"` → the branch is updated with its base branch.
- `mergeable == true` and `mergeable_state == "clean"` → the PR is eligible to merge,
  and is merged if it was created by you or approved by you.
- Any other state (e.g. `blocked`, `dirty`, `unstable`, `unknown`) is skipped.

Draft pull requests are always skipped.

## Docker Compose

```yaml
services:
  pr-watchdog:
    image: ghcr.io/ilteoood/pr-watchdog:latest
    restart: unless-stopped
    environment:
      GITHUB_TOKEN: ghp_xxx
      WATCHED_REPOS: "owner/repo1,owner/repo2"
      CRON_PATTERN: "0 */5 8-18 * * Mon-Fri *"
      RUST_LOG: info
```
