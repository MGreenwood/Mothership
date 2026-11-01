//! PowerShell adapter for dynamic completion.
//!
//! clap_complete's stock PowerShell script passes an empty current word as
//! `''`. Windows PowerShell 5.1 drops empty arguments to native programs, so
//! the cursor index shifts and `mothership beam <Tab>` completes the `beam`
//! subcommand instead. This adapter sends a sentinel in place of the empty
//! word and quotes names that contain whitespace. Other shells use the stock
//! adapters.

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;

use clap_complete::engine;
use clap_complete::env::EnvCompleter;

const EMPTY_WORD_SENTINEL: &str = "--mothership-empty-word";

const REGISTRATION: &str = r#"
Register-ArgumentCompleter -Native -CommandName __BIN__ -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @()
    foreach ($element in $commandAst.CommandElements) {
        if ($element.Extent.StartOffset -ge $cursorPosition) { break }
        if ($element -is [System.Management.Automation.Language.StringConstantExpressionAst]) {
            $words += $element.Value
        } else {
            $words += $element.Extent.Text
        }
    }
    if ($wordToComplete -eq '') { $words += '__SENTINEL__' }
    $previous = $env:__VAR__
    $env:__VAR__ = 'powershell'
    try {
        $results = & __COMPLETER__ -- @words 2>$null
    } finally {
        if ($null -eq $previous) {
            Remove-Item Env:\__VAR__ -ErrorAction SilentlyContinue
        } else {
            $env:__VAR__ = $previous
        }
    }
    foreach ($line in $results) {
        $parts = $line -split "`t", 2
        $name = $parts[0]
        $help = if ($parts.Count -gt 1) { $parts[1] } else { $name }
        $text = if ($name -match '\s') { "'" + $name.Replace("'", "''") + "'" } else { $name }
        [System.Management.Automation.CompletionResult]::new($text, $name, 'ParameterValue', $help)
    }
}
"#;

fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub struct PowerShellCompleter;

impl EnvCompleter for PowerShellCompleter {
    fn name(&self) -> &'static str {
        "powershell"
    }

    fn is(&self, name: &str) -> bool {
        matches!(name, "powershell" | "pwsh" | "powershell_ise")
    }

    fn write_registration(
        &self,
        var: &str,
        _name: &str,
        bin: &str,
        completer: &str,
        buf: &mut dyn Write,
    ) -> io::Result<()> {
        let script = REGISTRATION
            .replace("__BIN__", &ps_quote(bin))
            .replace("__COMPLETER__", &ps_quote(completer))
            .replace("__SENTINEL__", EMPTY_WORD_SENTINEL)
            .replace("__VAR__", var);
        writeln!(buf, "{script}")
    }

    fn write_complete(
        &self,
        cmd: &mut clap::Command,
        mut args: Vec<OsString>,
        current_dir: Option<&Path>,
        buf: &mut dyn Write,
    ) -> io::Result<()> {
        if let Some(last) = args.last_mut() {
            if last.to_str() == Some(EMPTY_WORD_SENTINEL) {
                *last = OsString::new();
            }
        }
        let index = args.len().saturating_sub(1);
        for candidate in engine::complete(cmd, args, index, current_dir)? {
            write!(buf, "{}", candidate.get_value().to_string_lossy())?;
            if let Some(help) = candidate.get_help() {
                write!(buf, "\t{}", help.to_string().lines().next().unwrap_or_default())?;
            }
            writeln!(buf)?;
        }
        Ok(())
    }
}
