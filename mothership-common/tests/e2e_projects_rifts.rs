mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use common::{spawn_test_server, TempDir, TEST_TOKEN};
use mothership_common::{
    auth::OAuthProvider,
    beam::{beam_project, read_project_metadata, restore_files_into},
    client::{scan_text_files, start_oauth_login, verify_oauth_token, MothershipClient},
    ignore::write_default_ignore_file,
};

#[tokio::test]
async fn e2e_auth_and_health_flow() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    client.health().await.unwrap();
    let max_size = client.max_upload_size().await.unwrap();
    assert_eq!(max_size, 1024 * 1024);

    let oauth = start_oauth_login(&url, OAuthProvider::Google).await.unwrap();
    assert_eq!(oauth.auth_url, "https://auth.example.test/login");

    verify_oauth_token(&url, TEST_TOKEN).await.unwrap();
    assert!(verify_oauth_token(&url, "wrong-token").await.is_err());

    let unauthed = MothershipClient::new(&url, Some("invalid".into())).unwrap();
    assert!(unauthed.list_projects().await.is_err());
}

#[tokio::test]
async fn e2e_gateway_create_scan_batch_upload_and_delete() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    let src_dir = TempDir::new("gateway-src");
    assert!(write_default_ignore_file(&src_dir.0).unwrap());
    // Second call returns false when .mothershipignore already exists
    assert!(!write_default_ignore_file(&src_dir.0).unwrap());

    std::fs::create_dir_all(src_dir.0.join("src")).unwrap();
    std::fs::write(src_dir.0.join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(src_dir.0.join(".env"), "SECRET=123").unwrap();
    std::fs::write(src_dir.0.join(".env.example"), "SECRET=").unwrap();
    std::fs::create_dir_all(src_dir.0.join("node_modules/pkg")).unwrap();
    std::fs::write(src_dir.0.join("node_modules/pkg/index.js"), "ignored").unwrap();
    std::fs::write(src_dir.0.join("binary.dat"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    std::fs::write(src_dir.0.join("huge.txt"), "x".repeat(2048)).unwrap();

    let report = scan_text_files(&src_dir.0, 1024).unwrap();
    assert!(report.files.contains_key(&PathBuf::from("src/main.rs")));
    assert!(report.files.contains_key(&PathBuf::from(".env.example")));
    assert!(!report.files.contains_key(&PathBuf::from(".env")));
    assert!(!report.files.contains_key(&PathBuf::from("node_modules/pkg/index.js")));
    assert_eq!(report.skipped_binary, vec![PathBuf::from("binary.dat")]);
    assert_eq!(report.skipped_too_large, vec![PathBuf::from("huge.txt")]);

    let project = client
        .create_gateway("orbital-core", "Core project", &src_dir.0)
        .await
        .unwrap();
    assert_eq!(project.name, "orbital-core");

    let uploaded = client.upload_files(project.id, report.files).await.unwrap();
    assert!(uploaded >= 2);

    let found = client.find_project("orbital-core").await.unwrap().unwrap();
    assert_eq!(found.id, project.id);

    client.delete_project(project.id).await.unwrap();
    assert!(client.find_project("orbital-core").await.unwrap().is_none());
}

#[tokio::test]
async fn e2e_beam_initial_websocket_download_and_stale_cleanup() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    let src_dir = TempDir::new("beam-src");
    let project = client
        .create_gateway("beam-app", "Beam test", &src_dir.0)
        .await
        .unwrap();

    let mut initial = HashMap::new();
    initial.insert(PathBuf::from("src/lib.rs"), "pub fn v() -> u32 { 1 }".into());
    initial.insert(PathBuf::from("README.md"), "# Beam App".into());
    client.upload_files(project.id, initial).await.unwrap();

    // Inject unsafe paths on join to verify the client rejects them
    state.inner.write().await.force_unsafe_path_on_join = true;

    let dest_dir = TempDir::new("beam-dest");
    std::fs::write(dest_dir.0.join("stale.txt"), "local stale file").unwrap();

    let outcome = beam_project(&client, &url, project.id, &project.name, &dest_dir.0, None)
        .await
        .unwrap();
    assert_eq!(outcome.downloaded, 2);
    assert_eq!(outcome.skipped_unsafe.len(), 2);
    assert_eq!(
        std::fs::read_to_string(dest_dir.0.join("src/lib.rs")).unwrap(),
        "pub fn v() -> u32 { 1 }"
    );
    // Stale file was removed from workspace and backed up in .mothership/local-backup
    assert!(!dest_dir.0.join("stale.txt").exists());
    assert!(dest_dir.0.join(".mothership/local-backup").exists());

    let meta = read_project_metadata(&dest_dir.0).unwrap();
    assert_eq!(meta.project_id, project.id.to_string());
    assert_eq!(meta.project_name, "beam-app");
    assert_eq!(meta.rift_id, Some(outcome.rift_id.to_string()));
}

#[tokio::test]
async fn e2e_rifts_fork_switch_checkpoints_and_restore() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    let dir = TempDir::new("rift-cp");
    let project = client
        .create_gateway("chrono-proj", "Checkpoint test", &dir.0)
        .await
        .unwrap();

    let mut v1_files = HashMap::new();
    v1_files.insert(PathBuf::from("config.toml"), "version = 1".into());
    client.upload_files(project.id, v1_files).await.unwrap();

    let cp1 = client.create_checkpoint(project.id, Some("v1 release")).await.unwrap();
    assert_eq!(cp1.file_count, 1);

    // Fork a feature rift from main
    let rifts = client.list_rifts(project.id).await.unwrap();
    let main_id = rifts.iter().find(|r| r.name == "main").unwrap().id;
    let feat_id = client
        .create_rift("feature-branch", Some("New work"), project.id, Some(main_id), false)
        .await
        .unwrap();
    assert_ne!(feat_id, main_id);

    // Switch to feature-branch and upload v2
    client.switch_rift(project.id, "feature-branch").await.unwrap();
    let mut v2_files = HashMap::new();
    v2_files.insert(PathBuf::from("config.toml"), "version = 2".into());
    v2_files.insert(PathBuf::from("extra.txt"), "extra".into());
    client.upload_files(project.id, v2_files).await.unwrap();
    let _cp2 = client.create_checkpoint(project.id, Some("v2 feature")).await.unwrap();

    let history = client.checkpoint_history(project.id).await.unwrap();
    assert_eq!(history.len(), 2);

    // Materialize v2 locally, then restore cp1 (v1)
    std::fs::write(dir.0.join("config.toml"), "version = 2 (local dirty)").unwrap();
    std::fs::write(dir.0.join("extra.txt"), "extra").unwrap();

    let restore_data = client.restore_checkpoint(project.id, cp1.checkpoint_id).await.unwrap();
    let restore_outcome = restore_files_into(&dir.0, &restore_data.files).unwrap();
    assert_eq!(restore_outcome.written, 1);
    assert_eq!(std::fs::read_to_string(dir.0.join("config.toml")).unwrap(), "version = 1");
    assert!(!dir.0.join("extra.txt").exists());
    // Local dirty version was backed up before overwrite
    assert!(dir.0.join(".mothership/local-backup").exists());
}
