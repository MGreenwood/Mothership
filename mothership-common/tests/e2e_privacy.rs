mod common;

use common::{spawn_test_server, MockServerState, TempDir, TEST_TOKEN};
use mothership_common::client::{MothershipClient, SwitchOutcome};
use uuid::Uuid;

async fn set_caller(state: &MockServerState, username: &str) {
    state.inner.write().await.caller = username.into();
}

async fn setup_private_rift() -> (MothershipClient, MockServerState, Uuid, Uuid) {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("privacy-proj");
    let project = client.create_gateway("private-proj", "Privacy test", &dir.0).await.unwrap();
    let secret_id = client.create_rift("secret", None, project.id, None, true).await.unwrap();
    (client, state, project.id, secret_id)
}

#[tokio::test]
async fn e2e_switch_reports_non_member_and_allows_member_after_grant() {
    let (client, state, project_id, _secret) = setup_private_rift().await;

    set_caller(&state, "bob").await;
    match client.switch_rift_outcome(project_id, "secret").await.unwrap() {
        SwitchOutcome::NotMember(_) => {}
        other => panic!("expected NotMember, got {other:?}"),
    }
    assert!(matches!(
        client.switch_rift_outcome(project_id, "main").await.unwrap(),
        SwitchOutcome::Switched(_)
    ));
    assert!(client.switch_rift(project_id, "missing").await.is_err());

    set_caller(&state, "tester").await;
    client.manage_rift_member(project_id, "secret", "bob", false).await.unwrap();

    set_caller(&state, "bob").await;
    match client.switch_rift_outcome(project_id, "secret").await.unwrap() {
        SwitchOutcome::Switched(name) => assert_eq!(name, "secret"),
        other => panic!("expected Switched, got {other:?}"),
    }
}

#[tokio::test]
async fn e2e_access_request_flow_owner_lists_and_approves() {
    let (client, state, project_id, _secret) = setup_private_rift().await;

    set_caller(&state, "bob").await;
    client.request_rift_access(project_id, "secret").await.unwrap();
    client.request_rift_access(project_id, "secret").await.unwrap();
    assert!(client.list_rift_access_requests(project_id, "secret").await.is_err());
    assert!(client.request_rift_access(project_id, "nope").await.is_err());

    set_caller(&state, "tester").await;
    let pending = client.list_rift_access_requests(project_id, "secret").await.unwrap();
    assert_eq!(pending.len(), 1, "repeated requests should not duplicate");
    assert_eq!(pending[0].username, "bob");

    client.manage_rift_member(project_id, "secret", "bob", false).await.unwrap();
    assert!(client.list_rift_access_requests(project_id, "secret").await.unwrap().is_empty());

    set_caller(&state, "bob").await;
    assert!(matches!(
        client.switch_rift_outcome(project_id, "secret").await.unwrap(),
        SwitchOutcome::Switched(_)
    ));
}

#[tokio::test]
async fn e2e_only_owner_manages_members_and_removal_revokes_access() {
    let (client, state, project_id, _secret) = setup_private_rift().await;

    set_caller(&state, "tester").await;
    client.manage_rift_member(project_id, "secret", "bob", false).await.unwrap();

    set_caller(&state, "bob").await;
    assert!(client.manage_rift_member(project_id, "secret", "carol", false).await.is_err());
    assert!(matches!(
        client.switch_rift_outcome(project_id, "secret").await.unwrap(),
        SwitchOutcome::Switched(_)
    ));

    set_caller(&state, "tester").await;
    client.manage_rift_member(project_id, "secret", "bob", true).await.unwrap();

    set_caller(&state, "bob").await;
    assert!(matches!(
        client.switch_rift_outcome(project_id, "secret").await.unwrap(),
        SwitchOutcome::NotMember(_)
    ));
}
