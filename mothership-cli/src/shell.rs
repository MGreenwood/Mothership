//! Interactive shell. Running `mothership` with no command reads commands from stdin until the
//! input ends (Ctrl+Z then Enter on Windows, Ctrl+D elsewhere) or the user types `exit`.

use anyhow::Result;
use clap::{CommandFactory, Parser};
use colored::Colorize;
use std::io::Write;

use crate::config::ConfigManager;
use crate::Cli;

pub async fn run(config_manager: &ConfigManager) -> Result<()> {
    println!("{}", "Mothership shell".cyan().bold());
    println!("Type a command such as 'gateway list'. 'help' lists commands. Ctrl+Z then Enter to leave.");

    loop {
        print!("mothership> ");
        std::io::stdout().flush()?;

        let Some(line) = tokio::task::spawn_blocking(read_line).await?? else {
            println!();
            break;
        };

        let mut tokens = match split_args(&line) {
            Ok(tokens) => tokens,
            Err(message) => {
                eprintln!("{}", message.red());
                continue;
            }
        };
        if tokens.first().is_some_and(|first| first == "mothership") {
            tokens.remove(0);
        }
        match tokens.first().map(String::as_str) {
            None => continue,
            Some("exit" | "quit") => break,
            Some("help") => {
                Cli::command().print_help()?;
                println!();
                continue;
            }
            Some(_) => {}
        }

        let argv = std::iter::once("mothership".to_string()).chain(tokens);
        match Cli::try_parse_from(argv) {
            Ok(Cli { command: Some(command) }) => {
                if let Err(error) = crate::run(command, config_manager).await {
                    eprintln!("{} {}", "Error:".red(), error);
                }
            }
            Ok(Cli { command: None }) => {}
            // Clap reports bad input and `--help` output here; the shell keeps going either way
            Err(error) => {
                let _ = error.print();
            }
        }
    }

    Ok(())
}

/// Reads one line. `None` means the input ended.
fn read_line() -> std::io::Result<Option<String>> {
    let mut buffer = String::new();
    let bytes = std::io::stdin().read_line(&mut buffer)?;
    Ok((bytes > 0).then_some(buffer))
}

/// Splits a line into arguments. Single or double quotes group words. Backslashes are kept as
/// written, so Windows paths such as `C:\Users\me` survive unchanged.
fn split_args(line: &str) -> Result<Vec<String>, String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;

    for c in line.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    args.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            (None, c) => {
                current.push(c);
                in_word = true;
            }
        }
    }

    if quote.is_some() {
        return Err("unclosed quote".to_string());
    }
    if in_word {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::split_args;

    #[test]
    fn splits_on_whitespace_and_collapses_runs() {
        assert_eq!(split_args("gateway   list  ").unwrap(), vec!["gateway", "list"]);
        assert!(split_args("   ").unwrap().is_empty());
    }

    #[test]
    fn quotes_group_words_and_are_removed() {
        assert_eq!(
            split_args(r#"init "My App" 'other name'"#).unwrap(),
            vec!["init", "My App", "other name"]
        );
        assert_eq!(split_args(r#"checkpoint -m """#).unwrap(), vec!["checkpoint", "-m", ""]);
    }

    #[test]
    fn backslashes_are_kept_for_windows_paths() {
        assert_eq!(
            split_args(r"beam proj --local-dir C:\Users\me\proj").unwrap(),
            vec!["beam", "proj", "--local-dir", r"C:\Users\me\proj"]
        );
    }

    #[test]
    fn unclosed_quote_is_an_error() {
        assert!(split_args(r#"init "oops"#).is_err());
    }
}
