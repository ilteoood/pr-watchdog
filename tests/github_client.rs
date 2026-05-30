use std::time::Duration;

use pr_watchdog::config::Repo;
use pr_watchdog::github::{GitHubClient, MergeMethod};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn repo() -> Repo {
    Repo {
        owner: "octo".to_string(),
        name: "demo".to_string(),
    }
}

/// Build a client pointed at the mock server with near-zero retry delays so
/// retry behaviour can be exercised without slowing the test suite down.
fn client(server: &MockServer) -> GitHubClient {
    GitHubClient::with_base_url(&server.uri(), "test-token", MergeMethod::Squash)
        .unwrap()
        .with_retry_settings(3, Duration::from_millis(1))
}

#[tokio::test]
async fn authenticated_login_returns_login() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octo" })))
        .mount(&server)
        .await;

    let login = client(&server).authenticated_login().await.unwrap();
    assert_eq!(login, "octo");
}

#[tokio::test]
async fn list_open_pull_requests_follows_pagination() {
    let server = MockServer::start().await;

    let first_page: Vec<_> = (1..=100)
        .map(|n| json!({ "number": n, "title": format!("pr {n}"), "draft": false }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(first_page))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "number": 101, "title": "last", "draft": true }])),
        )
        .mount(&server)
        .await;

    let prs = client(&server)
        .list_open_pull_requests(&repo())
        .await
        .unwrap();
    assert_eq!(prs.len(), 101);
    assert_eq!(prs[100].number, 101);
    assert!(prs[100].draft);
}

#[tokio::test]
async fn get_pull_request_parses_detail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/7"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "draft": false,
            "mergeable": true,
            "mergeable_state": "clean",
            "user": { "login": "octo" }
        })))
        .mount(&server)
        .await;

    let detail = client(&server).get_pull_request(&repo(), 7).await.unwrap();
    assert_eq!(detail.mergeable, Some(true));
    assert_eq!(detail.mergeable_state, "clean");
    assert_eq!(detail.user.login, "octo");
}

#[tokio::test]
async fn list_reviews_follows_pagination() {
    let server = MockServer::start().await;

    let first_page: Vec<_> = (0..100)
        .map(|_| json!({ "user": { "login": "octo" }, "state": "COMMENTED" }))
        .collect();
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/7/reviews"))
        .and(query_param("page", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(first_page))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/7/reviews"))
        .and(query_param("page", "2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!([{ "user": { "login": "octo" }, "state": "APPROVED" }])),
        )
        .mount(&server)
        .await;

    let reviews = client(&server).list_reviews(&repo(), 7).await.unwrap();
    assert_eq!(reviews.len(), 101);
    assert_eq!(reviews[100].state, "APPROVED");
}

#[tokio::test]
async fn merge_pull_request_sends_method() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/7/merge"))
        .and(body_json(json!({ "merge_method": "squash" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "merged": true })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server)
        .merge_pull_request(&repo(), 7)
        .await
        .unwrap();
}

#[tokio::test]
async fn update_branch_sends_put() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/repos/octo/demo/pulls/7/update-branch"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "message": "updating" })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server).update_branch(&repo(), 7).await.unwrap();
}

#[tokio::test]
async fn error_responses_are_mapped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/octo/demo/pulls/7"))
        .respond_with(ResponseTemplate::new(404).set_body_string("missing"))
        .mount(&server)
        .await;

    let err = client(&server)
        .get_pull_request(&repo(), 7)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("resource not found"), "got: {err}");
}

#[tokio::test]
async fn retries_on_rate_limit_then_succeeds() {
    let server = MockServer::start().await;
    // First call hits the secondary rate limit (403 + remaining 0), retried once.
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octo" })))
        .with_priority(2)
        .mount(&server)
        .await;

    let login = client(&server).authenticated_login().await.unwrap();
    assert_eq!(login, "octo");
}

#[tokio::test]
async fn retries_on_server_error_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octo" })))
        .with_priority(2)
        .mount(&server)
        .await;

    let login = client(&server).authenticated_login().await.unwrap();
    assert_eq!(login, "octo");
}

#[tokio::test]
async fn gives_up_after_exhausting_retries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let err = client(&server).authenticated_login().await.unwrap_err();
    assert!(err.to_string().contains("500"), "got: {err}");
}

#[tokio::test]
async fn sends_authorization_and_version_headers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .and(header("x-github-api-version", "2022-11-28"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "login": "octo" })))
        .expect(1)
        .mount(&server)
        .await;

    client(&server).authenticated_login().await.unwrap();
}
