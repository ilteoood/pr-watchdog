use std::time::Duration;

use pr_watchdog::config::Repo;
use pr_watchdog::github::{GitHubClient, MergeMethod};
use pr_watchdog::watcher::run_pass;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn repos() -> Vec<Repo> {
    vec![Repo {
        owner: "octo".to_string(),
        name: "demo".to_string(),
    }]
}

fn client(server: &MockServer) -> GitHubClient {
    GitHubClient::with_base_url(&server.uri(), "test-token", MergeMethod::Merge)
        .unwrap()
        .with_retry_settings(0, Duration::from_millis(1))
}

/// Mock the open-pull-requests list endpoint with a single PR.
async fn mount_open_prs(server: &MockServer, number: u64, draft: bool) {
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "number": number, "title": "a pr", "draft": draft }])),
        )
        .mount(server)
        .await;
}

async fn mount_pr_detail(
    server: &MockServer,
    number: u64,
    mergeable: Option<bool>,
    state: &str,
    author: &str,
) {
    Mock::given(method("GET"))
        .and(path(format!("/repos/octo/demo/pulls/{number}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "draft": false,
            "mergeable": mergeable,
            "mergeable_state": state,
            "user": { "login": author }
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn merges_pr_created_by_me_when_clean() {
    let server = MockServer::start().await;
    mount_open_prs(&server, 1, false).await;
    mount_pr_detail(&server, 1, Some(true), "clean", "me").await;
    let merge = Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/1/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "merged": true })))
        .expect(1)
        .named("merge")
        .mount_as_scoped(&server)
        .await;

    run_pass(&client(&server), &repos(), "me").await.unwrap();
    drop(merge); // verifies the merge endpoint was called exactly once
}

#[tokio::test]
async fn merges_pr_approved_by_me_when_clean() {
    let server = MockServer::start().await;
    mount_open_prs(&server, 2, false).await;
    mount_pr_detail(&server, 2, Some(true), "clean", "someone-else").await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/2/reviews"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "user": { "login": "me" }, "state": "APPROVED" }])),
        )
        .mount(&server)
        .await;
    let merge = Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/2/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "merged": true })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    run_pass(&client(&server), &repos(), "me").await.unwrap();
    drop(merge);
}

#[tokio::test]
async fn updates_branch_when_behind() {
    let server = MockServer::start().await;
    mount_open_prs(&server, 3, false).await;
    mount_pr_detail(&server, 3, None, "behind", "me").await;
    let update = Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/3/update-branch"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "message": "updating" })))
        .expect(1)
        .mount_as_scoped(&server)
        .await;

    run_pass(&client(&server), &repos(), "me").await.unwrap();
    drop(update);
}

#[tokio::test]
async fn skips_pr_not_mine_and_not_approved() {
    let server = MockServer::start().await;
    mount_open_prs(&server, 4, false).await;
    mount_pr_detail(&server, 4, Some(true), "clean", "someone-else").await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/4/reviews"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    // No merge mock mounted: if a merge were attempted it would 404 and surface
    // as a warning (run_pass still returns Ok), so assert no merge via a scoped
    // mock expecting zero calls.
    let merge = Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/4/merge"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    run_pass(&client(&server), &repos(), "me").await.unwrap();
    drop(merge);
}

#[tokio::test]
async fn skips_pr_when_not_mergeable() {
    let server = MockServer::start().await;
    mount_open_prs(&server, 5, false).await;
    mount_pr_detail(&server, 5, Some(false), "dirty", "me").await;
    let merge = Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/5/merge"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    run_pass(&client(&server), &repos(), "me").await.unwrap();
    drop(merge);
}

#[tokio::test]
async fn skips_draft_pull_requests() {
    let server = MockServer::start().await;
    mount_open_prs(&server, 6, true).await;
    // A draft PR must not even be fetched in detail.
    let detail = Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/6"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount_as_scoped(&server)
        .await;

    run_pass(&client(&server), &repos(), "me").await.unwrap();
    drop(detail);
}
