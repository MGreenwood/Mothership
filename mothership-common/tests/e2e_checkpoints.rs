mod common;

use std::path::PathBuf;

use common::{spawn_test_server, TempDir, TEST_TOKEN};
use mothership_common::client::MothershipClient;

#[tokio::test]
async fn e2e_scoped_checkpoint_captures_the_selection_and_restores_the_folded_state() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("checkpoint-proj");
    let project = client.create_gateway("checkpoint-proj", "Checkpoint test", &dir.0).await.unwrap();
    let rift = *state.inner.read().await.active_rift_by_project.get(&project.id).unwrap();

    let set_files = |pairs: &[(&str, &str)]| {
        let state = state.clone();
        let pairs: Vec<(PathBuf, String)> = pairs
            .iter()
            .map(|(p, c)| (PathBuf::from(p), c.to_string()))
            .collect();
        async move {
            let mut inner = state.inner.write().await;
            let files = &mut inner.rifts.get_mut(&rift).unwrap().files;
            for (path, content) in pairs {
                files.insert(path, content);
            }
        }
    };

    set_files(&[("README.md", "v1"), ("src/main.rs", "v1")]).await;
    let whole = client.create_checkpoint(project.id, Some("base"), None).await.unwrap();
    assert_eq!(whole.file_count, 2);

    set_files(&[("README.md", "v2"), ("src/main.rs", "v2")]).await;
    let partial = client
        .create_checkpoint(project.id, Some("src only"), Some(&["src/".to_string()]))
        .await
        .unwrap();
    assert_eq!(partial.file_count, 1, "only the folder's file is captured");

    // Restoring the partial checkpoint gives src/ at the new version and README.md at the base version
    let restored = client.restore_checkpoint(project.id, partial.checkpoint_id, None).await.unwrap();
    assert_eq!(restored.files.get(&PathBuf::from("src/main.rs")).map(String::as_str), Some("v2"));
    assert_eq!(restored.files.get(&PathBuf::from("README.md")).map(String::as_str), Some("v1"));

    // Paths that match nothing are refused and nothing is recorded
    let missing = client
        .create_checkpoint(project.id, None, Some(&["nope.rs".to_string()]))
        .await;
    assert!(missing.is_err());
    assert_eq!(client.checkpoint_history(project.id).await.unwrap().len(), 2);
}

#[tokio::test]
async fn e2e_partial_restore_returns_only_the_selection() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("restore-paths");
    let project = client.create_gateway("restore-paths", "Restore paths", &dir.0).await.unwrap();
    let rift = *state.inner.read().await.active_rift_by_project.get(&project.id).unwrap();

    {
        let mut inner = state.inner.write().await;
        let files = &mut inner.rifts.get_mut(&rift).unwrap().files;
        files.insert(PathBuf::from("README.md"), "v1".to_string());
        files.insert(PathBuf::from("src/main.rs"), "v1".to_string());
    }
    let whole = client.create_checkpoint(project.id, Some("base"), None).await.unwrap();

    let selected = client
        .restore_checkpoint(project.id, whole.checkpoint_id, Some(&["src/".to_string()]))
        .await
        .unwrap();
    assert_eq!(selected.files.len(), 1);
    assert_eq!(selected.files.get(&PathBuf::from("src/main.rs")).map(String::as_str), Some("v1"));

    let everything = client.restore_checkpoint(project.id, whole.checkpoint_id, None).await.unwrap();
    assert_eq!(everything.files.len(), 2);

    let missing = client
        .restore_checkpoint(project.id, whole.checkpoint_id, Some(&["nope.rs".to_string()]))
        .await;
    assert!(missing.is_err());
}

#[tokio::test]
async fn e2e_partial_checkpoint_needs_an_earlier_checkpoint() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("checkpoint-first");
    let project = client.create_gateway("checkpoint-first", "First checkpoint", &dir.0).await.unwrap();

    let result = client
        .create_checkpoint(project.id, None, Some(&["README.md".to_string()]))
        .await;
    assert!(result.is_err(), "the first checkpoint of a rift must be whole-rift");
}
