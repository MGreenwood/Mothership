//! Ignore rules for scanning a local project directory.
//!
//! Rules live in a `.mothershipignore` file at the root of the tracked directory,
//! written in gitignore syntax. `mothership init` creates it with defaults. If the
//! file is missing, the same defaults are used in memory.

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const IGNORE_FILE_NAME: &str = ".mothershipignore";

/// Directory name that is always excluded, regardless of ignore rules.
const ALWAYS_IGNORED_DIR: &str = ".mothership";

pub const DEFAULT_IGNORE_TEMPLATE: &str = r#"# Mothership ignore file (gitignore syntax).
# Files matching these patterns are not uploaded by `mothership init`.
# Delete this file and run `mothership init` again to get the defaults back.

# Version control
.git/
.svn/
.hg/

# Secrets and credentials
.env
.env.*
!.env.example
*.pem
*.key
*.p12
*.pfx
*.keystore
*.jks
id_rsa
id_rsa.*
id_ed25519
id_ed25519.*
id_ecdsa
id_ecdsa.*
*.ppk
credentials
credentials.*
secrets.*
.npmrc
.pypirc
.netrc
.htpasswd
.aws/
.ssh/
.gnupg/
terraform.tfstate
terraform.tfstate.*

# Compiled binaries and archives
*.exe
*.dll
*.so
*.dylib
*.bin
*.o
*.obj
*.pdb
*.class
*.jar
*.pyc
*.pyo
*.wasm
*.zip
*.tar
*.tar.gz
*.tgz
*.7z
*.rar

# Build and dependency output
/build/
/dist/
/target/
node_modules/
__pycache__/
.venv/
venv/
*.log
*.tmp
*.temp
.cache/

# OS and editor files
*~
*.swp
*.swo
.DS_Store
Thumbs.db
desktop.ini
.vscode/
.idea/
"#;

/// Writes the default `.mothershipignore` into `root` if none exists.
/// Returns `true` when the file was created.
pub fn write_default_ignore_file(root: &Path) -> Result<bool> {
    let path = root.join(IGNORE_FILE_NAME);
    match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(mut file) => {
            file.write_all(DEFAULT_IGNORE_TEMPLATE.as_bytes())
                .with_context(|| format!("failed to write {}", path.display()))?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e).with_context(|| format!("failed to create {}", path.display())),
    }
}

/// Compiled ignore rules for one project root.
pub struct IgnoreRules {
    root: PathBuf,
    gitignore: Gitignore,
}

impl IgnoreRules {
    /// Loads `.mothershipignore` from `root`, or the built-in defaults if it is absent.
    pub fn load(root: &Path) -> Result<Self> {
        let mut builder = GitignoreBuilder::new(root);
        let path = root.join(IGNORE_FILE_NAME);
        if path.is_file() {
            if let Some(err) = builder.add(&path) {
                return Err(err).with_context(|| format!("failed to read {}", path.display()));
            }
        } else {
            for line in DEFAULT_IGNORE_TEMPLATE.lines() {
                builder.add_line(None, line)?;
            }
        }
        let gitignore = builder.build().context("invalid rule in .mothershipignore")?;
        Ok(Self {
            root: root.to_path_buf(),
            gitignore,
        })
    }

    /// Returns true if `path` (inside the root) should not be uploaded or synced.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        if relative.as_os_str().is_empty() {
            return false;
        }
        if relative
            .components()
            .any(|c| c.as_os_str() == ALWAYS_IGNORED_DIR)
        {
            return true;
        }
        self.gitignore
            .matched_path_or_any_parents(relative, is_dir)
            .is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules_from(lines: &str) -> IgnoreRules {
        let root = PathBuf::from("proj");
        let mut builder = GitignoreBuilder::new(&root);
        for line in lines.lines() {
            builder.add_line(None, line).unwrap();
        }
        IgnoreRules {
            root,
            gitignore: builder.build().unwrap(),
        }
    }

    #[test]
    fn anchored_pattern_only_matches_root() {
        let rules = rules_from("/build/");
        assert!(rules.is_ignored(Path::new("proj/build/out.js"), false));
        assert!(!rules.is_ignored(Path::new("proj/src/build/out.js"), false));
    }

    #[test]
    fn unanchored_name_does_not_match_substrings() {
        let rules = rules_from("build/\ndist/");
        assert!(!rules.is_ignored(Path::new("proj/rebuild.md"), false));
        assert!(!rules.is_ignored(Path::new("proj/distance.rs"), false));
        assert!(rules.is_ignored(Path::new("proj/app/build"), true));
    }

    #[test]
    fn negation_keeps_example_env() {
        let rules = rules_from(".env\n.env.*\n!.env.example");
        assert!(rules.is_ignored(Path::new("proj/.env"), false));
        assert!(rules.is_ignored(Path::new("proj/.env.production"), false));
        assert!(!rules.is_ignored(Path::new("proj/.env.example"), false));
    }

    #[test]
    fn ignored_directory_excludes_children() {
        let rules = rules_from("node_modules/");
        assert!(rules.is_ignored(Path::new("proj/web/node_modules/x/index.js"), false));
    }

    #[test]
    fn mothership_metadata_is_always_ignored() {
        let rules = rules_from("");
        assert!(rules.is_ignored(Path::new("proj/.mothership/project.json"), false));
        assert!(!rules.is_ignored(Path::new("proj/src/main.rs"), false));
    }

    #[test]
    fn root_itself_is_never_ignored() {
        let rules = rules_from("*");
        assert!(!rules.is_ignored(Path::new("proj"), true));
    }

    #[test]
    fn default_template_parses() {
        let rules = rules_from(DEFAULT_IGNORE_TEMPLATE);
        assert!(rules.is_ignored(Path::new("proj/app.exe"), false));
        assert!(rules.is_ignored(Path::new("proj/.git/HEAD"), false));
        assert!(!rules.is_ignored(Path::new("proj/src/main.rs"), false));
    }
}
