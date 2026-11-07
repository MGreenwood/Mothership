//! `mothership secrets`: review files held back because they contain a secret, then allow or keep them.

use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;
use mothership_common::secrets::{allow_held_file, pending_holds, SecretFinding};

pub fn handle_secrets() -> Result<()> {
    let project = std::env::current_dir()?;
    let holds = pending_holds(&project);
    if holds.is_empty() {
        println!("No files are held back for secrets.");
        return Ok(());
    }

    println!("{}", format!("{} file(s) held back because they contain a secret:", holds.len()).yellow().bold());
    for (path, findings) in holds {
        println!("\n{}", path.bold());
        for line in describe_findings(&findings) {
            println!("  {}", line);
        }
        if prompt("Allow syncing this secret? (y/N): ")? {
            allow_held_file(&project, &path)?;
            println!("{}", format!("Allowed. {} will sync now.", path).green());
        } else {
            println!("{}", format!("{} stays on this machine. Remove the secret and it syncs automatically.", path).yellow());
        }
    }
    Ok(())
}

/// One line per finding. The secret itself is never shown.
pub fn describe_findings(findings: &[SecretFinding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| format!("line {}: {}", finding.line, finding.kind))
        .collect()
}

fn prompt(question: &str) -> Result<bool> {
    print!("{}", question);
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::describe_findings;
    use mothership_common::secrets::SecretFinding;

    #[test]
    fn describes_lines_and_kinds_without_the_secret() {
        let findings = vec![SecretFinding {
            kind: "GitHub token".to_string(),
            line: 7,
            fingerprint: "abc".to_string(),
        }];
        assert_eq!(describe_findings(&findings), vec!["line 7: GitHub token"]);
    }
}
