mod common;

use std::path::PathBuf;

use common::{spawn_test_server, TempDir, TEST_TOKEN};
use mothership_common::client::MothershipClient;
use mothership_common::text_diff::DiffStatus;

#[tokio::test]
async fn e2e_rift_diff_lists_changes_and_checkpoint_sides_work() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("diff-proj");
    let project = client.create_gateway("diff-proj", "Diff test", &dir.0).await.unwrap();
    let main = *state.inner.read().await.active_rift_by_project.get(&project.id).unwrap();
    let feature = client.create_rift("feature", None, project.id, None, false).await.unwrap();

    {
        let mut inner = state.inner.write().await;
        let files = &mut inner.rifts.get_mut(&main).unwrap().files;
        files.insert(PathBuf::from("README.md"), "v1\n".to_string());
        files.insert(PathBuf::from("gone.txt"), "bye".to_string());
        let files = &mut inner.rifts.get_mut(&feature).unwrap().files;
        files.insert(PathBuf::from("README.md"), "v2\n".to_string());
        files.insert(PathBuf::from("new.txt"), "hi".to_string());
    }

    let diffs = client.rift_diff("main", "feature", false).await.unwrap();
    let status_of = |path: &str| diffs.iter().find(|d| d.path == path).map(|d| d.status);
    assert_eq!(status_of("README.md"), Some(DiffStatus::Modified));
    assert_eq!(status_of("gone.txt"), Some(DiffStatus::Removed));
    assert_eq!(status_of("new.txt"), Some(DiffStatus::Added));
    assert!(diffs.iter().all(|d| d.unified.is_none()));

    // The file view has the unified text, with both versions visible
    let file = client.rift_file_diff("main", "feature", "README.md").await.unwrap();
    let unified = file.unified.unwrap();
    assert!(unified.contains("-v1") && unified.contains("+v2"), "{unified}");

    // A checkpoint is a side too: what changed in main since the checkpoint was taken
    let checkpoint = client.create_checkpoint(project.id, Some("base"), None).await.unwrap();
    state.inner.write().await.rifts.get_mut(&main).unwrap().files.insert(
        PathBuf::from("README.md"),
        "v3\n".to_string(),
    );
    let since = client
        .rift_diff(&format!("checkpoint:{}", checkpoint.checkpoint_id), "main", true)
        .await
        .unwrap();
    let readme = since.iter().find(|d| d.path == "README.md").expect("README.md changed");
    assert!(readme.unified.as_deref().unwrap().contains("+v3"));

    assert!(client.rift_diff("main", "no-such-rift", false).await.is_err());
}
