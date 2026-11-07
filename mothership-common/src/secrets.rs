//! Secret scanning before sync. A file that contains a secret nobody has allowed is held back: its
//! content never leaves this machine, while the rest of the rift keeps syncing.
//!
//! Holds and allowances live in `.mothership/`, which is never synced. They store fingerprints
//! and line numbers, never the secret itself.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::normalize_rel_path;

pub const HOLDS_FILE: &str = "secret-holds.json";
pub const ALLOWED_FILE: &str = "allowed-secrets.json";
const METADATA_DIR: &str = ".mothership";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretFinding {
    /// What kind of secret was found, such as "AWS access key ID"
    pub kind: String,
    /// Line number, starting at 1
    pub line: u32,
    /// Hash of the secret. Used to remember allowances without storing the secret.
    pub fingerprint: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HoldsFile {
    /// Held files by project-relative path
    #[serde(default)]
    pub files: BTreeMap<String, Vec<SecretFinding>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedFile {
    #[serde(default)]
    pub fingerprints: BTreeSet<String>,
}

/// What the daemon should do with a file before it leaves the machine
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretGate {
    Send,
    Hold(Vec<SecretFinding>),
}

struct Rule {
    kind: &'static str,
    pattern: Regex,
    /// Capture group holding the secret value; the whole match when `None`
    value_group: Option<usize>,
    /// Skip values that are obviously placeholders, such as `API_KEY=changeme`
    check_placeholder: bool,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            Rule {
                kind: "AWS access key ID",
                pattern: Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("valid pattern"),
                value_group: None,
                check_placeholder: false,
            },
            Rule {
                kind: "GitHub token",
                pattern: Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{36,}\b").expect("valid pattern"),
                value_group: None,
                check_placeholder: false,
            },
            Rule {
                kind: "Slack token",
                pattern: Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}").expect("valid pattern"),
                value_group: None,
                check_placeholder: false,
            },
            Rule {
                kind: "private key",
                pattern: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").expect("valid pattern"),
                value_group: None,
                check_placeholder: false,
            },
            Rule {
                kind: "secret value in an assignment",
                pattern: Regex::new(
                    r#"(?i)\b[A-Z0-9_]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_?KEY|PRIVATE_?KEY)[A-Z0-9_]*\s*[=:]\s*["']?([^\s"'#;,]{8,})"#,
                )
                .expect("valid pattern"),
                value_group: Some(1),
                check_placeholder: true,
            },
        ]
    })
}

fn is_placeholder(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    const MARKERS: [&str; 7] = ["changeme", "example", "placeholder", "your", "xxxx", "${", "<"];
    MARKERS.iter().any(|marker| lower.contains(marker))
        || value.chars().all(|c| c == '*' || c == '.')
}

fn fingerprint(kind: &str, value: &str) -> String {
    let digest = Sha256::digest(format!("{kind}\0{value}").as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Every secret in `content`, with its line number. The secret text itself is not returned.
pub fn scan_text(content: &str) -> Vec<SecretFinding> {
    let mut findings = Vec::new();
    for (index, line) in content.lines().enumerate() {
        for rule in rules() {
            for captures in rule.pattern.captures_iter(line) {
                let value = match rule.value_group {
                    Some(group) => captures.get(group),
                    None => captures.get(0),
                }
                .map(|m| m.as_str())
                .unwrap_or_default();
                if value.is_empty() || (rule.check_placeholder && is_placeholder(value)) {
                    continue;
                }
                findings.push(SecretFinding {
                    kind: rule.kind.to_string(),
                    line: (index + 1) as u32,
                    fingerprint: fingerprint(rule.kind, value),
                });
            }
        }
    }
    findings
}

fn metadata_path(project: &Path, file: &str) -> PathBuf {
    project.join(METADATA_DIR).join(file)
}

fn read_json<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(value)?)
        .with_context(|| format!("Failed to write {}", path.display()))
}

pub fn load_holds(project: &Path) -> HoldsFile {
    read_json(&metadata_path(project, HOLDS_FILE))
}

pub fn load_allowed(project: &Path) -> AllowedFile {
    read_json(&metadata_path(project, ALLOWED_FILE))
}

fn save_holds(project: &Path, holds: &HoldsFile) -> Result<()> {
    write_json(&metadata_path(project, HOLDS_FILE), holds)
}

fn save_allowed(project: &Path, allowed: &AllowedFile) -> Result<()> {
    write_json(&metadata_path(project, ALLOWED_FILE), allowed)
}

/// Decides whether a changed file may be sent. A file with unallowed secrets is held, and the
/// hold is recorded. A clean file clears any earlier hold.
pub fn gate_file(project: &Path, rel: &Path, content: &str) -> SecretGate {
    let key = normalize_rel_path(rel);
    let allowed = load_allowed(project);
    let blocked: Vec<SecretFinding> = scan_text(content)
        .into_iter()
        .filter(|finding| !allowed.fingerprints.contains(&finding.fingerprint))
        .collect();

    let mut holds = load_holds(project);
    if blocked.is_empty() {
        if holds.files.remove(&key).is_some() {
            // Best effort: a failed write only leaves a stale hold that the next clean save clears
            let _ = save_holds(project, &holds);
        }
        return SecretGate::Send;
    }

    holds.files.insert(key, blocked.clone());
    let _ = save_holds(project, &holds);
    SecretGate::Hold(blocked)
}

/// Files currently held back, with their findings, sorted by path
pub fn pending_holds(project: &Path) -> Vec<(String, Vec<SecretFinding>)> {
    load_holds(project).files.into_iter().collect()
}

/// Allows the secrets that hold a file back, then re-saves the file so the daemon sends it.
pub fn allow_held_file(project: &Path, rel: &str) -> Result<()> {
    let key = normalize_rel_path(Path::new(rel));
    let mut holds = load_holds(project);
    let Some(findings) = holds.files.remove(&key) else {
        bail!("{} is not held back by a secret", key);
    };

    let mut allowed = load_allowed(project);
    allowed.fingerprints.extend(findings.into_iter().map(|f| f.fingerprint));
    save_allowed(project, &allowed)?;
    save_holds(project, &holds)?;

    // Writing the same bytes back makes the file watcher report a change, so the daemon sends it now
    let path = project.join(&key);
    let content = fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    fs::write(&path, content).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn project() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mothership-secrets-{}", Uuid::new_v4()));
        fs::create_dir_all(dir.join(METADATA_DIR)).unwrap();
        dir
    }

    fn kinds(content: &str) -> Vec<String> {
        scan_text(content).into_iter().map(|f| f.kind).collect()
    }

    #[test]
    fn detects_the_built_in_secret_kinds() {
        assert_eq!(kinds("key = AKIAABCDEFGHIJKLMNOP"), vec!["AWS access key ID"]);
        assert_eq!(kinds(&format!("token ghp_{}", "a".repeat(36))), vec!["GitHub token"]);
        assert_eq!(kinds("-----BEGIN RSA PRIVATE KEY-----"), vec!["private key"]);
        assert_eq!(kinds("DB_PASSWORD=hunter2hunter2"), vec!["secret value in an assignment"]);
    }

    #[test]
    fn placeholders_and_plain_text_are_not_secrets() {
        assert!(scan_text("API_KEY=changeme-please").is_empty());
        assert!(scan_text("TOKEN=${TOKEN}").is_empty());
        assert!(scan_text("the password field is required").is_empty());
    }

    #[test]
    fn findings_carry_line_numbers_and_never_the_secret() {
        let findings = scan_text("fine\nfine\nDB_PASSWORD=hunter2hunter2\n");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 3);
        assert!(!format!("{:?}", findings[0]).contains("hunter2"));
    }

    #[test]
    fn a_secret_holds_the_file_until_it_is_allowed() {
        let dir = project();
        let rel = Path::new("config/app.env");
        let leaked = "DB_PASSWORD=hunter2hunter2\n";

        assert!(matches!(gate_file(&dir, rel, leaked), SecretGate::Hold(_)));
        assert_eq!(pending_holds(&dir).len(), 1);

        fs::create_dir_all(dir.join("config")).unwrap();
        fs::write(dir.join(rel), leaked).unwrap();
        allow_held_file(&dir, "config/app.env").unwrap();

        assert!(pending_holds(&dir).is_empty());
        assert_eq!(gate_file(&dir, rel, leaked), SecretGate::Send, "the same secret stays allowed");
        assert!(matches!(
            gate_file(&dir, rel, "DB_PASSWORD=different-secret-value\n"),
            SecretGate::Hold(_)
        ), "a changed secret needs a new decision");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn removing_the_secret_releases_the_file_without_allowing_it() {
        let dir = project();
        let rel = Path::new("app.env");
        assert!(matches!(gate_file(&dir, rel, "DB_PASSWORD=hunter2hunter2\n"), SecretGate::Hold(_)));

        assert_eq!(gate_file(&dir, rel, "DB_PASSWORD=${DB_PASSWORD}\n"), SecretGate::Send);
        assert!(pending_holds(&dir).is_empty());
        assert!(load_allowed(&dir).fingerprints.is_empty());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_held_files_can_be_allowed() {
        let dir = project();
        assert!(allow_held_file(&dir, "nothing.txt").is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
