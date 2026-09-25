// Suppress the console window on Windows — without this the OS opens a CMD
// prompt alongside the GUI process.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::Result;

/// Release version of Aura. Passed down so `aura --version`, the modal and
/// the update check all report the release tag, not a library crate's version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<()> {
    // Subcommand dispatch — handled before GPUI init so headless commands
    // don't spin up a window/event loop. `aura` with no subcommand falls
    // through to the tray entry point.
    let cli = aura_cli::Cli::parse_with_version(VERSION);
    if let Some(command) = cli.command {
        return aura_cli::dispatch(command);
    }
    aura_ui::run(VERSION)
}
