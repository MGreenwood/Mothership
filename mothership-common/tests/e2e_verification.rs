mod common;

use common::{spawn_test_server, TempDir, TEST_TOKEN};
use mothership_common::client::MothershipClient;
use mothership_common::verify::{self, CheckResult};
use uuid::Uuid;

const CONFIG: &str = r#"
[[check]]
name = "passes"
run = "exit 0"

[[check]]
name = "fails"
run = "exit 2"
"#;

#[tokio::test]
async fn e2e_checkpoint_checks_are_recorded_and_attached() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("verify-proj");
    std::fs::write(dir.0.join(verify::CONFIG_FILE), CONFIG).unwrap();
    let project = client.create_gateway("verify-proj", "Verification test", &dir.0).await.unwrap();
    client.create_rift("main", None, project.id, None, false).await.unwrap();
    let checkpoint = client.create_checkpoint(project.id, Some("with checks"), None).await.unwrap();

    let result = verify::run_and_record(&client, project.id, &dir.0, Some(checkpoint.checkpoint_id))
        .await
        .unwrap()
        .expect("the project has a config");

    assert!(!result.passed);
    let codes: Vec<(&str, Option<i32>)> = result
        .checks
        .iter()
        .map(|c: &CheckResult| (c.name.as_str(), c.exit_code))
        .collect();
    assert_eq!(codes, vec![("passes", Some(0)), ("fails", Some(2))]);

    let inner = state.inner.read().await;
    let reports = inner.verifications.get(&project.id).expect("the result was posted");
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].checkpoint_id, Some(checkpoint.checkpoint_id));
    let (stored, _) = inner
        .checkpoints
        .get(&project.id)
        .and_then(|list| list.iter().find(|(cp, _)| cp.id == checkpoint.checkpoint_id))
        .expect("the checkpoint exists");
    assert_eq!(stored.verification.as_ref().map(|v| v.passed), Some(false));
}

#[tokio::test]
async fn e2e_projects_without_a_config_run_nothing() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("verify-none");
    let project = client.create_gateway("verify-none", "No checks", &dir.0).await.unwrap();

    let result = verify::run_and_record(&client, project.id, &dir.0, None).await.unwrap();

    assert!(result.is_none());
    assert!(state.inner.read().await.verifications.get(&project.id).is_none());
}

#[tokio::test]
async fn e2e_an_invalid_config_is_reported_and_nothing_is_recorded() {
    let (url, state) = spawn_test_server().await;
    let client = MothershipClient::new(&url, Some(TEST_TOKEN.to_string())).unwrap();
    let dir = TempDir::new("verify-bad");
    std::fs::write(dir.0.join(verify::CONFIG_FILE), "[[check]]\nname = \"no command\"\n").unwrap();
    let project = client.create_gateway("verify-bad", "Bad config", &dir.0).await.unwrap();

    let outcome = verify::run_and_record(&client, project.id, &dir.0, Some(Uuid::new_v4())).await;

    assert!(outcome.is_err());
    assert!(state.inner.read().await.verifications.get(&project.id).is_none());
}
