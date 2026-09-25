//! `aura update` — self-update via the install / uninstall scripts.
//!
//! Mirrors `warren update`: in an Aura source checkout (cwd has `install.sh`,
//! `uninstall.sh`, and a `Cargo.toml`) this runs the local scripts —
//! `./uninstall.sh` then `./install.sh` — rebuilding from source. Anywhere
//! else it streams the scripts from GitHub (`curl … | bash`), the README's
//! two-curl flow. Either way, `args` are forwarded to both scripts so a
//! `--components <subset>` selection narrows the uninstall as well as the
//! install; `uninstall.sh` ignores install-only flags it doesn't understand.

use std::path::Path;
use std::process::Command as Proc;

use anyhow::{anyhow, bail, Result};
use clap::Args;

/// Raw `main`-branch URLs of the install/uninstall scripts, used when `aura
/// update` runs outside a source checkout (the two-curl flow from the README).
const UNINSTALL_URL: &str = "https://raw.githubusercontent.com/Rfluid/aura/main/uninstall.sh";
const INSTALL_URL: &str = "https://raw.githubusercontent.com/Rfluid/aura/main/install.sh";

#[derive(Debug, Args)]
pub struct UpdateCli {
    /// Extra arguments forwarded verbatim to both the uninstall and install
    /// scripts (e.g. `--mode release`, `--version v1.2.3`, or
    /// `--components tray,cli` to update only a subset).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}

impl UpdateCli {
    pub fn run(self) -> Result<()> {
        let cwd = std::env::current_dir()?;
        let in_checkout = cwd.join("install.sh").is_file()
            && cwd.join("uninstall.sh").is_file()
            && cwd.join("Cargo.toml").is_file();

        // `uninstall.sh` stops the running tray with `pkill -x aura`, which
        // also targets *this* `aura update` process — the CLI and tray share
        // the `aura` binary name. Without this, the uninstall half kills us
        // before the install half runs, leaving the machine with no `aura`.
        // Ignoring SIGTERM lets the process survive the pkill and finish the
        // reinstall.
        ignore_sigterm();

        if in_checkout {
            println!(
                "aura update: source checkout detected — running ./uninstall.sh then ./install.sh"
            );
            run(Proc::new("bash")
                .arg("./uninstall.sh")
                .args(&self.args)
                .current_dir(&cwd))?;
            run(Proc::new("bash")
                .arg("./install.sh")
                .args(&self.args)
                .current_dir(&cwd))?;
        } else {
            println!("aura update: fetching install scripts from GitHub…");
            curl_bash(UNINSTALL_URL, &self.args, &cwd)?;
            curl_bash(INSTALL_URL, &self.args, &cwd)?;
        }
        println!("✔ aura update complete.");
        Ok(())
    }
}

/// Stream a script from `url` into bash, forwarding `args` to it, run in `cwd`.
/// `$0` is the URL and `$@` the forwarded args, so the piped script sees them
/// as its own positional arguments.
fn curl_bash(url: &str, args: &[String], cwd: &Path) -> Result<()> {
    let mut cmd = Proc::new("bash");
    cmd.arg("-c")
        .arg(r#"curl -fsSL "$0" | bash -s -- "$@""#)
        .arg(url)
        .args(args)
        .current_dir(cwd);
    run(&mut cmd)
}

/// Set SIGTERM to be ignored so `uninstall.sh`'s `pkill -x aura` can't abort
/// this process midway through the update. Best-effort; only the reinstall
/// window needs the protection and the process exits shortly after.
#[cfg(unix)]
fn ignore_sigterm() {
    // SAFETY: installing SIG_IGN for SIGTERM is async-signal-safe and has no
    // memory-safety implications; the return value (previous handler) is
    // intentionally discarded.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

/// No-op on non-Unix targets — the update path shells out to `bash`, so it
/// only runs on Unix anyway.
#[cfg(not(unix))]
fn ignore_sigterm() {}

/// Run a child to completion, mapping a non-zero exit to an error.
fn run(cmd: &mut Proc) -> Result<()> {
    let status = cmd
        .status()
        .map_err(|e| anyhow!("failed to run {:?}: {e}", cmd.get_program()))?;
    if !status.success() {
        bail!("`{:?}` exited with {status}", cmd.get_program());
    }
    Ok(())
}
