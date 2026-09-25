//! `aura theme …` subcommands.
//!
//! `theme path` prints the resolved on-disk location, `theme init` seeds
//! the bundled defaults (the same content the modal's "Themes" entry
//! writes on first click), and `theme edit` opens it in `$EDITOR`.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use aura_core::theme::Theme;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct ThemeCli {
    #[command(subcommand)]
    command: ThemeCommand,
}

#[derive(Debug, Subcommand)]
enum ThemeCommand {
    /// Print the theme file path.
    Path,
    /// Open `theme.toml` in `$EDITOR` (seeds defaults if missing).
    Edit,
    /// Write the default theme.toml to disk.
    Init {
        /// Overwrite an existing theme file.
        #[arg(long)]
        force: bool,
    },
}

impl ThemeCli {
    pub fn run(self) -> Result<()> {
        match self.command {
            ThemeCommand::Path => {
                println!("{}", Theme::default_path().display());
                Ok(())
            }
            ThemeCommand::Edit => {
                let path = Theme::default_path();
                if !path.exists() {
                    seed_default(&path)?;
                }
                open_in_editor(&path)
            }
            ThemeCommand::Init { force } => {
                let path = Theme::default_path();
                if path.exists() && !force {
                    println!(
                        "{} already exists (pass --force to overwrite).",
                        path.display()
                    );
                    return Ok(());
                }
                seed_default(&path)?;
                println!("Wrote {}", path.display());
                Ok(())
            }
        }
    }
}

fn seed_default(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create theme dir {}", parent.display()))?;
    }
    std::fs::write(path, Theme::DEFAULT_TOML).with_context(|| format!("write {}", path.display()))
}

/// Spawn `$EDITOR` (fallback: `$VISUAL`, then `vi`/`notepad`) on `path`.
/// Returns when the editor exits. Shared with `config edit`.
pub fn open_in_editor(path: &Path) -> Result<()> {
    use std::process::Command;
    let editor = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        });

    let status = Command::new(&editor)
        .arg(path)
        .status()
        .with_context(|| format!("spawn editor `{editor}`"))?;
    if !status.success() {
        return Err(anyhow!("editor `{editor}` exited with status {status}"));
    }
    Ok(())
}
