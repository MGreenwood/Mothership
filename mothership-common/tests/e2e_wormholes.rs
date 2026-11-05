mod common;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use common::{spawn_test_server, TempDir, TEST_TOKEN};
use futures_util::StreamExt;
use mothership_common::{
    beam::{beam_wormhole_files, WormholeMarker},
    client::MothershipClient,
    merge::Choice,
    protocol::SyncMessage,
};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn e2e_wormhole_conflict_resolution_and_converge() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    let dir = TempDir::new("wh-converge");
    let project = client.create_gateway("wh-proj", "Wormhole test", &dir.0).await.unwrap();

    // Base files on `main`
    let mut base_files = HashMap::new();
    base_files.insert(PathBuf::from("shared.rs"), "line1\nline2\nline3\nline4\nline5".into());
    base_files.insert(PathBuf::from("conflict.rs"), "header\nbody\nfooter".into());
    client.upload_files(project.id, base_files).await.unwrap();

    let main_id = client.list_rifts(project.id).await.unwrap()[0].id;
    let alice_id = client.create_rift("alice", None, project.id, Some(main_id), false).await.unwrap();
    let bob_id = client.create_rift("bob", None, project.id, Some(main_id), false).await.unwrap();

    // Create the wormhole while alice and bob share the common base from `main`
    let wh = client
        .create_wormhole(project.id, "release-preview", &[alice_id, bob_id], false)
        .await
        .unwrap();
    assert_eq!(wh.unresolved_conflicts, 0);

    // Alice edits top of shared.rs and middle of conflict.rs
    client.switch_rift(project.id, "alice").await.unwrap();
    let mut alice_files = HashMap::new();
    alice_files.insert(PathBuf::from("shared.rs"), "ALICE1\nline2\nline3\nline4\nline5".into());
    alice_files.insert(PathBuf::from("conflict.rs"), "header\nalice_body\nfooter".into());
    client.upload_files(project.id, alice_files).await.unwrap();

    // Bob edits bottom of shared.rs (non-overlapping!) and middle of conflict.rs (overlapping!)
    client.switch_rift(project.id, "bob").await.unwrap();
    let mut bob_files = HashMap::new();
    bob_files.insert(PathBuf::from("shared.rs"), "line1\nline2\nline3\nline4\nBOB5".into());
    bob_files.insert(PathBuf::from("conflict.rs"), "header\nbob_body\nfooter".into());
    client.upload_files(project.id, bob_files).await.unwrap();

    let detail = client.get_wormhole(wh.id).await.unwrap();
    assert_eq!(detail.summary.unresolved_conflicts, 1);

    // Beaming or converging while conflicts remain must fail (409)
    assert!(client.wormhole_files(wh.id).await.is_err());
    assert!(client.converge_wormhole(wh.id, None).await.is_err());

    // Per-side diff endpoint works even while conflicts remain
    let rift_files = client.wormhole_rift_files(wh.id).await.unwrap();
    assert_eq!(rift_files.len(), 2);

    // Resolve the conflict by taking side 0
    let detail = client.get_wormhole(wh.id).await.unwrap();
    let conflict = &detail.files[0].conflicts[0];
    let resolved = client
        .resolve_wormhole_conflict(wh.id, "conflict.rs", &conflict.id, Choice::Side(0))
        .await
        .unwrap();
    assert_eq!(resolved.summary.unresolved_conflicts, 0);

    // Converge into a new rift
    let converged = client
        .converge_wormhole(wh.id, Some("merged-v1"))
        .await
        .unwrap();
    assert_eq!(converged.rift_name, "merged-v1");
    assert_eq!(converged.files_written, 2);

    let after = client.get_wormhole(wh.id).await.unwrap();
    assert_eq!(after.summary.status, "converged");
    assert_eq!(after.summary.converted_rift_id, Some(converged.rift_id));

    // Also verify creating a wormhole AFTER rifts have already diverged from `main`
    // uses `main` as the fallback base so non-overlapping edits still merge cleanly.
    let post_diverge_wh = client
        .create_wormhole(project.id, "post-diverge-preview", &[alice_id, bob_id], false)
        .await
        .unwrap();
    assert_eq!(post_diverge_wh.unresolved_conflicts, 1);
    let post_detail = client.get_wormhole(post_diverge_wh.id).await.unwrap();
    assert_eq!(post_detail.files.len(), 1);
    assert_eq!(post_detail.files[0].path, "conflict.rs");

    // Resolve with Choice::Custom (replaces the conflicted hunk at line 2)
    let cid = post_detail.files[0].conflicts[0].id.clone();
    let custom_resolved = client
        .resolve_wormhole_conflict(
            post_diverge_wh.id,
            "conflict.rs",
            &cid,
            Choice::Custom(vec!["alice_and_bob_body".into()]),
        )
        .await
        .unwrap();
    assert_eq!(custom_resolved.summary.unresolved_conflicts, 0);
    let merged_files = client.wormhole_files(post_diverge_wh.id).await.unwrap();
    let by_path: HashMap<String, String> = merged_files.into_iter().map(|f| (f.path, f.content)).collect();
    assert_eq!(by_path["shared.rs"], "ALICE1\nline2\nline3\nline4\nBOB5");
    assert_eq!(by_path["conflict.rs"], "header\nalice_and_bob_body\nfooter");
}

#[tokio::test]
async fn e2e_wormhole_modular_rift_toggle_and_live_watch() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    let dir = TempDir::new("wh-modular");
    let project = client.create_gateway("modular-app", "Modular test", &dir.0).await.unwrap();

    let mut base_files = HashMap::new();
    base_files.insert(PathBuf::from("app.rs"), "fn init() {}\nfn run() {}".into());
    client.upload_files(project.id, base_files).await.unwrap();

    let main_id = client.list_rifts(project.id).await.unwrap()[0].id;
    let alice_id = client.create_rift("alice-auth", None, project.id, Some(main_id), false).await.unwrap();
    let bob_id = client.create_rift("bob-payments", None, project.id, Some(main_id), false).await.unwrap();
    let carol_id = client.create_rift("carol-ui", None, project.id, Some(main_id), false).await.unwrap();

    // Create the live integration wormhole across the three developer rifts
    let wh = client
        .create_wormhole(project.id, "dev-board", &[alice_id, bob_id, carol_id], false)
        .await
        .unwrap();
    assert_eq!(wh.unresolved_conflicts, 0);

    // Alice modifies line 1 of app.rs and adds auth.rs
    client.switch_rift(project.id, "alice-auth").await.unwrap();
    client
        .upload_files(
            project.id,
            HashMap::from([
                (PathBuf::from("app.rs"), "fn init() { auth(); }\nfn run() {}".into()),
                (PathBuf::from("auth.rs"), "pub fn auth() {}".into()),
            ]),
        )
        .await
        .unwrap();

    // Bob modifies line 1 of app.rs (conflicting with Alice!) and adds payments.rs
    client.switch_rift(project.id, "bob-payments").await.unwrap();
    client
        .upload_files(
            project.id,
            HashMap::from([
                (PathBuf::from("app.rs"), "fn init() { broken_pay(); }\nfn run() {}".into()),
                (PathBuf::from("payments.rs"), "pub fn pay() {}".into()),
            ]),
        )
        .await
        .unwrap();

    // Carol adds ui.rs cleanly
    client.switch_rift(project.id, "carol-ui").await.unwrap();
    client
        .upload_files(
            project.id,
            HashMap::from([(PathBuf::from("ui.rs"), "pub fn render() {}".into())]),
        )
        .await
        .unwrap();

    let detail_with_conflict = client.get_wormhole(wh.id).await.unwrap();
    assert_eq!(detail_with_conflict.summary.unresolved_conflicts, 1);

    // Connect a live WebSocket watcher to `/ws/wormholes/:id`
    let ws_url = format!("{}/ws/wormholes/{}", url.replace("http://", "ws://"), wh.id);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url).await.unwrap();

    // Toggle Bob's broken/conflicting rift OFF
    let toggled = client
        .toggle_wormhole_rift(wh.id, bob_id, Some(false))
        .await
        .unwrap();
    assert_eq!(toggled.summary.unresolved_conflicts, 0);
    assert!(toggled.summary.disabled_rift_ids.contains(&bob_id));
    let bob_side = toggled.sides.iter().find(|s| s.rift_id == bob_id).unwrap();
    assert!(!bob_side.enabled);

    // Watcher receives WormholeUpdated notification
    let ws_msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if let Message::Text(txt) = ws_msg {
        let parsed: SyncMessage = serde_json::from_str(&txt).unwrap();
        assert!(matches!(parsed, SyncMessage::WormholeUpdated { wormhole_id } if wormhole_id == wh.id));
    } else {
        panic!("expected text websocket message");
    }

    // Beam the clean wormhole (Alice + Carol active, Bob muted) into a local folder
    let files = client.wormhole_files(wh.id).await.unwrap();
    let beam_folder = dir.0.join("beamed-board");
    let marker = WormholeMarker {
        wormhole_id: wh.id.to_string(),
        wormhole_name: wh.name.clone(),
        project_id: project.id.to_string(),
    };
    let pairs: Vec<(String, String)> = files.into_iter().map(|f| (f.path, f.content)).collect();
    beam_wormhole_files(&beam_folder, &marker, &pairs).unwrap();

    assert!(beam_folder.join("auth.rs").exists());
    assert!(beam_folder.join("ui.rs").exists());
    assert!(!beam_folder.join("payments.rs").exists());
    assert_eq!(
        std::fs::read_to_string(beam_folder.join("app.rs")).unwrap(),
        "fn init() { auth(); }\nfn run() {}"
    );

    // Now toggle Carol OFF as well and re-beam into the same folder: `ui.rs` must be removed!
    client.toggle_wormhole_rift(wh.id, carol_id, Some(false)).await.unwrap();
    let files2 = client.wormhole_files(wh.id).await.unwrap();
    let pairs2: Vec<(String, String)> = files2.into_iter().map(|f| (f.path, f.content)).collect();
    beam_wormhole_files(&beam_folder, &marker, &pairs2).unwrap();
    assert!(beam_folder.join("auth.rs").exists());
    assert!(!beam_folder.join("ui.rs").exists());

    // Toggle Bob back ON (flip with None): conflict returns as expected
    let bob_back = client.toggle_wormhole_rift(wh.id, bob_id, None).await.unwrap();
    assert_eq!(bob_back.summary.unresolved_conflicts, 1);
}

#[tokio::test]
async fn e2e_wormhole_dissolve_and_permanent_delete() {
    let (url, _state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();

    let dir = TempDir::new("wh-delete");
    let project = client.create_gateway("del-proj", "Delete test", &dir.0).await.unwrap();
    let main_id = client.list_rifts(project.id).await.unwrap()[0].id;
    let r2 = client.create_rift("r2", None, project.id, Some(main_id), false).await.unwrap();

    let wh = client
        .create_wormhole(project.id, "temp-wormhole", &[main_id, r2], false)
        .await
        .unwrap();

    // Standard dissolve archives the wormhole (still present in list with status == "dissolved")
    client.dissolve_wormhole(wh.id).await.unwrap();
    let list_after_dissolve = client.list_wormholes(project.id).await.unwrap();
    assert_eq!(list_after_dissolve.len(), 1);
    assert_eq!(list_after_dissolve[0].status, "dissolved");

    // Permanent delete removes it from the server completely
    client.delete_wormhole(wh.id).await.unwrap();
    let list_after_delete = client.list_wormholes(project.id).await.unwrap();
    assert!(list_after_delete.is_empty());
    assert!(client.get_wormhole(wh.id).await.is_err());
}
