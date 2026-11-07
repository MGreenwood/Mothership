//! Optional checks on checkpoints. A project's `cp-config.toml` lists commands that run from the
//! project folder when a checkpoint is saved. A check passes when its command exits with code 0.

use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::client::MothershipClient;

pub const CONFIG_FILE: &str = "cp-config.toml";

#[derive(Debug, Deserialize, PartialEq)]
pub struct CheckConfig {
    #[serde(default)]
    pub check: Vec<Check>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Check {
    pub name: String,
    pub run: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    /// None when the command could not start or was ended by a signal
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationResult {
    pub passed: bool,
    pub checks: Vec<CheckResult>,
    pub ran_at: DateTime<Utc>,
}

/// Body of `POST /projects/:id/verifications`. Without a checkpoint id the result is only logged.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerificationReport {
    pub checkpoint_id: Option<Uuid>,
    pub result: VerificationResult,
}

/// Parses a config and checks it: unique names, and a name and command for each check. A config with no
/// checks is valid and means nothing runs, so a commented-out template is harmless.
pub fn parse_config(text: &str) -> Result<CheckConfig> {
    let config: CheckConfig = toml::from_str(text).context("cp-config.toml is not valid TOML")?;
    let mut names = HashSet::new();
    for check in &config.check {
        if check.name.trim().is_empty() {
            return Err(anyhow!("a check in cp-config.toml has an empty name"));
        }
        if check.run.trim().is_empty() {
            return Err(anyhow!("check '{}' has an empty run command", check.name));
        }
        if !names.insert(check.name.as_str()) {
            return Err(anyhow!("check name '{}' is used more than once", check.name));
        }
    }
    Ok(config)
}

/// Reads `cp-config.toml` from the project root. Returns None when the project has no config.
pub fn load_config(project_root: &Path) -> Result<Option<CheckConfig>> {
    let path = project_root.join(CONFIG_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse_config(&text).map(Some)
}

/// Runs every check in order from the project root. The commands' output goes to this process's
/// output, so a CLI user sees their test output. Only the exit codes decide the result.
pub async fn run_checks(project_root: &Path, config: &CheckConfig) -> VerificationResult {
    let root = project_root.to_path_buf();
    let checks = config.check.clone();
    let results = tokio::task::spawn_blocking(move || {
        checks.iter().map(|check| run_one(&root, check)).collect::<Vec<_>>()
    })
    .await
    .unwrap_or_else(|_| {
        config
            .check
            .iter()
            .map(|check| CheckResult { name: check.name.clone(), passed: false, exit_code: None })
            .collect()
    });

    VerificationResult {
        passed: results.iter().all(|check| check.passed),
        checks: results,
        ran_at: Utc::now(),
    }
}

fn run_one(root: &Path, check: &Check) -> CheckResult {
    let status = shell_command(&check.run)
        .current_dir(root)
        .stdin(Stdio::null())
        .status();
    let exit_code = status.ok().and_then(|status| status.code());
    CheckResult {
        name: check.name.clone(),
        passed: exit_code == Some(0),
        exit_code,
    }
}

fn shell_command(run: &str) -> Command {
    if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(run);
        command
    } else {
        let mut command = Command::new("sh");
        command.arg("-c").arg(run);
        command
    }
}

/// Runs the project's checks and records the result on the server. Returns None when the project has
/// no `cp-config.toml`. An invalid config is an error, so callers can report it without failing the
/// checkpoint.
pub async fn run_and_record(
    client: &MothershipClient,
    project_id: Uuid,
    project_root: &Path,
    checkpoint_id: Option<Uuid>,
) -> Result<Option<VerificationResult>> {
    let Some(config) = load_config(project_root)? else {
        return Ok(None);
    };
    if config.check.is_empty() {
        return Ok(None);
    }
    let result = run_checks(project_root, &config).await;
    client
        .record_verification(
            project_id,
            VerificationReport { checkpoint_id, result: result.clone() },
        )
        .await?;
    Ok(Some(result))
}

/// A starting `cp-config.toml` with every example commented out, so it runs nothing until edited
pub const CONFIG_TEMPLATE: &str = r#"# Checks run from the project folder after each checkpoint, and by `mothership verify`.
# A check passes when its command exits with code 0. Its output shows in your terminal.
# Keep checks fast: they run every time a checkpoint is saved.
# With no [[check]] entries, nothing runs. Uncomment the examples you want.

# [[check]]
# name = "unit tests"
# run = "cargo test"

# [[check]]
# name = "lint"
# run = "cargo clippy -- -D warnings"

# [[check]]
# name = "frontend build"
# run = "npm run build"

# [[check]]
# name = "python tests"
# run = "python -m pytest"
"#;

/// Writes the template as `cp-config.toml` in the project root. An existing file is left alone.
/// Returns whether a file was written.
pub fn write_template_if_missing(project_root: &Path) -> Result<bool> {
    let path = project_root.join(CONFIG_FILE);
    if path.exists() {
        return Ok(false);
    }
    std::fs::write(&path, CONFIG_TEMPLATE)
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_checks_in_order() {
        let config = parse_config(
            r#"
            [[check]]
            name = "unit tests"
            run = "cargo test"

            [[check]]
            name = "lint"
            run = "cargo clippy"
            "#,
        )
        .unwrap();
        let names: Vec<&str> = config.check.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["unit tests", "lint"]);
        assert_eq!(config.check[1].run, "cargo clippy");
    }

    #[test]
    fn rejects_configs_with_bad_entries() {
        assert!(parse_config("[[check]]\nname = \"x\"\n").is_err());
        assert!(parse_config("[[check]]\nname = \" \"\nrun = \"true\"\n").is_err());
        assert!(parse_config("[[check]]\nname = \"x\"\nrun = \"\"\n").is_err());
        assert!(parse_config("[[check]]\nname = \"x\"\nrun = \"a\"\n[[check]]\nname = \"x\"\nrun = \"b\"\n").is_err());
        assert!(parse_config("not = [valid").is_err());
    }

    #[test]
    fn a_config_with_no_checks_is_valid_and_empty() {
        assert!(parse_config("").unwrap().check.is_empty());
        assert!(parse_config(CONFIG_TEMPLATE).unwrap().check.is_empty());
    }

    #[test]
    fn template_is_written_once_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("verify-template-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        assert!(write_template_if_missing(&dir).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap(), CONFIG_TEMPLATE);

        std::fs::write(dir.join(CONFIG_FILE), "[[check]]\nname = \"mine\"\nrun = \"exit 0\"\n").unwrap();
        assert!(!write_template_if_missing(&dir).unwrap());
        assert!(std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap().contains("mine"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_config_file_means_no_checks() {
        let dir = std::env::temp_dir().join(format!("verify-missing-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_config(&dir).unwrap().is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn exit_code_zero_passes_and_anything_else_fails() {
        let config = parse_config(
            "[[check]]\nname = \"ok\"\nrun = \"exit 0\"\n\n[[check]]\nname = \"bad\"\nrun = \"exit 3\"\n",
        )
        .unwrap();

        let result = run_checks(&std::env::temp_dir(), &config).await;
        assert!(!result.passed);
        assert_eq!(
            result.checks,
            vec![
                CheckResult { name: "ok".into(), passed: true, exit_code: Some(0) },
                CheckResult { name: "bad".into(), passed: false, exit_code: Some(3) },
            ]
        );
    }

    #[tokio::test]
    async fn all_passing_checks_pass_the_checkpoint() {
        let config = parse_config("[[check]]\nname = \"ok\"\nrun = \"exit 0\"\n").unwrap();
        let result = run_checks(&std::env::temp_dir(), &config).await;
        assert!(result.passed);
    }

    #[tokio::test]
    async fn a_command_that_cannot_start_fails_without_an_exit_code() {
        let config = parse_config(
            "[[check]]\nname = \"missing\"\nrun = \"this-program-does-not-exist-anywhere --flag\"\n",
        )
        .unwrap();
        let result = run_checks(&std::env::temp_dir(), &config).await;
        assert!(!result.passed);
        assert!(!result.checks[0].passed);
    }
}
