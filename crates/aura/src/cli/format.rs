//! Shared output-format primitives.
//!
//! Read commands accept `--format text|json`. JSON is for scripting
//! (jq, status-bar widgets, dashboards); text is the default and is
//! intended for humans reading directly in a terminal.

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
}

pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let s = serde_json::to_string_pretty(value).context("serialize JSON output")?;
    println!("{s}");
    Ok(())
}
