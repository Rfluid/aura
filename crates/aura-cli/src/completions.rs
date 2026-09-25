//! `aura completions <shell>` — emit a clap_complete shell-completion script.
//!
//! Send the output into your shell's completion path, e.g.:
//!
//! ```text
//! aura completions zsh > ~/.zfunc/_aura
//! aura completions bash > /etc/bash_completion.d/aura
//! ```

use std::io::{self, Write};

use anyhow::{Context, Result};
use clap::{Args, CommandFactory, ValueEnum};
use clap_complete::{generate, Shell};

#[derive(Debug, Args)]
pub struct CompletionsCli {
    /// Target shell.
    shell: ShellArg,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ShellArg {
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}

impl From<ShellArg> for Shell {
    fn from(s: ShellArg) -> Self {
        match s {
            ShellArg::Bash => Shell::Bash,
            ShellArg::Zsh => Shell::Zsh,
            ShellArg::Fish => Shell::Fish,
            ShellArg::Powershell => Shell::PowerShell,
            ShellArg::Elvish => Shell::Elvish,
        }
    }
}

impl CompletionsCli {
    pub fn run<C: CommandFactory>(self) -> Result<()> {
        let mut cmd = C::command();
        let name = cmd.get_name().to_string();
        // clap_complete's `generate` panics on EPIPE when the receiving
        // pipe closes early (e.g. `aura completions zsh | head`). Buffer
        // the script first so the panicking write is to an in-memory Vec,
        // then handle SIGPIPE the usual way via `write_all` on stdout.
        let mut buf: Vec<u8> = Vec::new();
        generate(Shell::from(self.shell), &mut cmd, name, &mut buf);
        let mut out = io::stdout().lock();
        if let Err(e) = out.write_all(&buf) {
            if e.kind() == io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(e).context("write completion script to stdout");
        }
        Ok(())
    }
}
