mod common;

use chrono::Utc;
use common::{spawn_test_server, TempDir, TEST_TOKEN};
use mothership_common::client::{MothershipClient, RiftEvent, RiftEventQuery};
use serde_json::json;
use uuid::Uuid;

fn event(id: i64, content: &str) -> RiftEvent {
    RiftEvent {
        id,
        event_type: "chat".to_string(),
        actor_id: None,
        actor_username: Some("tester".to_string()),
        payload: json!({ "content": content }),
        created_at: Utc::now(),
    }
}

fn contents(events: &[RiftEvent]) -> Vec<&str> {
    events
        .iter()
        .map(|e| e.payload["content"].as_str().unwrap())
        .collect()
}

fn query(after: i64, before: Option<i64>, latest: bool, limit: i64) -> RiftEventQuery {
    RiftEventQuery { after, before, latest, limit, wait_secs: 0 }
}

#[tokio::test]
async fn e2e_rift_events_page_forward_backward_and_latest() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("events-proj");
    let project = client.create_gateway("events-proj", "Events test", &dir.0).await.unwrap();
    let rift = client.create_rift("notes", None, project.id, None, false).await.unwrap();

    state.inner.write().await.rift_events.insert(
        rift,
        vec![event(1, "one"), event(2, "two"), event(3, "three")],
    );

    let forward = client.rift_events(rift, &query(1, None, false, 200)).await.unwrap();
    assert_eq!(contents(&forward), vec!["two", "three"]);

    let newest = client.rift_events(rift, &query(0, None, true, 2)).await.unwrap();
    assert_eq!(contents(&newest), vec!["two", "three"]);

    let older = client.rift_events(rift, &query(0, Some(3), false, 10)).await.unwrap();
    assert_eq!(contents(&older), vec!["one", "two"]);
}

#[tokio::test]
async fn e2e_rift_events_are_refused_to_non_collaborators() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("events-private");
    let project = client.create_gateway("events-private", "Private events", &dir.0).await.unwrap();
    let secret: Uuid = client.create_rift("secret", None, project.id, None, true).await.unwrap();
    state.inner.write().await.rift_events.insert(secret, vec![event(1, "hidden")]);

    // Someone outside the private rift cannot read its log
    state.inner.write().await.caller = "bob".to_string();
    assert!(client.rift_events(secret, &query(0, None, false, 200)).await.is_err());

    // The owner can
    state.inner.write().await.caller = "tester".to_string();
    let owner_view = client.rift_events(secret, &query(0, None, false, 200)).await.unwrap();
    assert_eq!(contents(&owner_view), vec!["hidden"]);
}
