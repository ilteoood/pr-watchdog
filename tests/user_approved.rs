use pr_watchdog::github::user_approved;
use pr_watchdog::github::Review;
use pr_watchdog::github::User;

fn user(login: &str) -> Option<User> {
    Some(User {
        login: login.to_string(),
    })
}

fn review(login: &str, state: &str) -> Review {
    Review {
        user: user(login),
        state: state.to_string(),
    }
}

fn make_reviews(reviews: &[(&str, &str)]) -> Vec<Review> {
    reviews
        .iter()
        .map(|(login, state)| review(login, state))
        .collect()
}

#[test]
fn user_approved_no_reviews() {
    assert!(!user_approved(&[], "alice"));
}

#[test]
fn user_approved_only_comments() {
    let reviews = make_reviews(&[("alice", "COMMENTED")]);
    assert!(!user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_multiple_reviews_takes_latest_approved() {
    // Alice first commented, then approved — latest state wins
    let reviews = make_reviews(&[("alice", "COMMENTED"), ("alice", "APPROVED")]);
    assert!(user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_changes_requested_after_approved() {
    // Alice approved then requested changes — approved is overridden
    let reviews = make_reviews(&[("alice", "APPROVED"), ("alice", "CHANGES_REQUESTED")]);
    assert!(!user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_dismissed_after_approved() {
    let reviews = make_reviews(&[("alice", "APPROVED"), ("alice", "DISMISSED")]);
    assert!(!user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_approved_after_changes_requested() {
    // Later approval wins over earlier changes request
    let reviews = make_reviews(&[("alice", "CHANGES_REQUESTED"), ("alice", "APPROVED")]);
    assert!(user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_only_other_user() {
    let reviews = make_reviews(&[("bob", "APPROVED")]);
    assert!(!user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_ignores_other_users() {
    // Only Alice's reviews matter for "alice"; Bob's APPROVED is irrelevant
    let reviews = make_reviews(&[
        ("bob", "APPROVED"),
        ("alice", "COMMENTED"),
        ("carol", "CHANGES_REQUESTED"),
    ]);
    assert!(!user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_pending_is_ignored() {
    let reviews = make_reviews(&[("alice", "PENDING")]);
    assert!(!user_approved(&reviews, "alice"));
}

#[test]
fn user_approved_dismissed_is_ignored_for_other_users() {
    // Dismissed only clears approval for the same user
    let reviews = make_reviews(&[("alice", "DISMISSED"), ("bob", "APPROVED")]);
    assert!(!user_approved(&reviews, "alice"));
    assert!(user_approved(&reviews, "bob"));
}
