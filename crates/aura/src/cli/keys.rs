//! `aura keys …` subcommands for the modal's keyboard shortcuts.
//!
//! The read side mirrors `aura config`: `describe` explains actions (or what
//! a keystroke does), `get` looks one keystroke up, `list` prints the
//! effective keymap, `validate` reports problems. The write side edits
//! `keybindings.toml` in place through `aura_core::keymap::file::KeymapFile`,
//! which keeps the user's comments and layout: `set`, `unbind`, `reset`,
//! `wizard`, `merge`. `init`, `export` and `document` generate whole files.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use aura_core::{
    config::AppConfig,
    keymap::{
        file::{KeymapFile, MergePrefer, MergeReport},
        parse_action, parse_keys, ActionGroup, Binding, BindingContext, BindingSource, KeyAction,
        Keymap, KeymapWarning,
    },
};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use super::format::{print_json, OutputFormat};
use super::theme::open_in_editor;

#[derive(Debug, Args)]
pub struct KeysCli {
    #[command(subcommand)]
    command: KeysCommand,
}

/// `--context` values.
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum ContextArg {
    /// Applies everywhere.
    #[default]
    Global,
    /// Checked first while a menu, the settings panel or the help is open.
    Overlay,
}

impl From<ContextArg> for BindingContext {
    fn from(c: ContextArg) -> Self {
        match c {
            ContextArg::Global => BindingContext::Global,
            ContextArg::Overlay => BindingContext::Overlay,
        }
    }
}

/// `merge --prefer` values.
#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum PreferArg {
    /// The incoming file wins conflicts.
    #[default]
    Theirs,
    /// Your file wins conflicts.
    Ours,
}

#[derive(Debug, Subcommand)]
enum KeysCommand {
    /// Print the keybindings file path.
    Path,
    /// Print the effective keymap: defaults merged with keybindings.toml.
    List {
        /// Only this context.
        #[arg(long, value_enum)]
        context: Option<ContextArg>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// List every action with its keys — or explain one action (`scroll_down`)
    /// or keystroke (`G`, `"g g"`).
    #[command(alias = "actions")]
    Describe {
        /// An action name or a keystroke to explain (omit to list everything).
        target: Option<String>,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Print what a keystroke does (e.g. `get G`, `get "g g"`).
    Get {
        keys: String,
        #[arg(long, value_enum, default_value_t = ContextArg::Global)]
        context: ContextArg,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Bind a keystroke to an action and save (e.g. `set ctrl-j scroll_down`).
    Set {
        keys: String,
        /// Action name, or `none` to unbind.
        action: String,
        #[arg(long, value_enum, default_value_t = ContextArg::Global)]
        context: ContextArg,
    },
    /// Remove a keystroke's binding, including a default (`= "none"`).
    Unbind {
        keys: String,
        #[arg(long, value_enum, default_value_t = ContextArg::Global)]
        context: ContextArg,
    },
    /// Drop your overrides so the defaults apply again: for one keystroke,
    /// one action (`--action`), or everything (`--all`).
    Reset {
        /// Keystroke whose entry to remove.
        #[arg(conflicts_with_all = ["action", "all"])]
        keys: Option<String>,
        /// Restore this action's default keys.
        #[arg(long, conflicts_with = "all")]
        action: Option<String>,
        /// Remove every entry (broken ones are left for you to fix).
        #[arg(long)]
        all: bool,
        /// Context to reset. Defaults to global, or to both with `--all`.
        #[arg(long, value_enum)]
        context: Option<ContextArg>,
    },
    /// Interactively walk every action, keeping its keys on blank input.
    Wizard {
        #[arg(long, value_enum, default_value_t = ContextArg::Global)]
        context: ContextArg,
    },
    /// Fold another keybindings file into yours (`-` reads stdin).
    Merge {
        file: PathBuf,
        /// Who wins when both files bind the same keys differently.
        #[arg(long, value_enum, default_value_t = PreferArg::Theirs)]
        prefer: PreferArg,
        /// Report what would change and exit non-zero if anything would,
        /// without touching your file.
        #[arg(long)]
        check: bool,
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Print the effective keymap as a self-contained keybindings.toml
    /// (`use_defaults = false`, every binding written out).
    Export,
    /// Write a starter keybindings.toml listing every default as a comment.
    Init {
        /// Overwrite an existing keybindings file.
        #[arg(long)]
        force: bool,
        /// Write every default as a live binding with `use_defaults = false`,
        /// instead of a commented reference.
        #[arg(long)]
        full: bool,
    },
    /// Rewrite keybindings.toml in the generated layout, keeping every valid
    /// entry and adding a description to each.
    Document {
        /// Rewrite even if some entries can't be carried over (they are dropped).
        #[arg(long)]
        force: bool,
    },
    /// Check keybindings.toml and report problems. Exits 1 if there are any.
    Validate {
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },
    /// Open keybindings.toml in `$EDITOR` (seeds it if missing).
    Edit,
}

impl KeysCli {
    pub fn run(self) -> Result<()> {
        let path = Keymap::default_path();
        match self.command {
            KeysCommand::Path => {
                println!("{}", path.display());
                Ok(())
            }
            KeysCommand::List { context, format } => {
                run_list(&path, context.map(Into::into), format)
            }
            KeysCommand::Describe { target, format } => {
                run_describe(&path, target.as_deref(), format)
            }
            KeysCommand::Get {
                keys,
                context,
                format,
            } => run_get(&path, &keys, context.into(), format),
            KeysCommand::Set {
                keys,
                action,
                context,
            } => run_set(&path, &keys, &action, context.into()),
            KeysCommand::Unbind { keys, context } => run_set(&path, &keys, "none", context.into()),
            KeysCommand::Reset {
                keys,
                action,
                all,
                context,
            } => run_reset(&path, keys, action, all, context.map(Into::into)),
            KeysCommand::Wizard { context } => run_wizard(&path, context.into()),
            KeysCommand::Merge {
                file,
                prefer,
                check,
                format,
            } => run_merge(&path, &file, prefer, check, format),
            KeysCommand::Export => {
                let file = KeymapFile::load(&path)?;
                let keymap = file.keymap();
                print_warnings_stderr(&keymap.warnings);
                print!("{}", keymap.to_explicit_toml());
                Ok(())
            }
            KeysCommand::Init { force, full } => run_init(&path, force, full),
            KeysCommand::Document { force } => run_document(&path, force),
            KeysCommand::Validate { format } => run_validate(&path, format),
            KeysCommand::Edit => {
                if !path.exists() {
                    KeymapFile::default().save(&path)?;
                }
                open_in_editor(&path)?;
                print_warnings_stderr(&Keymap::load(&path).warnings);
                Ok(())
            }
        }
    }
}

// ── read ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ListReport<'a> {
    /// `[keybindings] enabled` from config.toml.
    enabled: bool,
    path: String,
    bindings: Vec<&'a Binding>,
    warnings: &'a [KeymapWarning],
}

fn run_list(path: &Path, context: Option<BindingContext>, format: OutputFormat) -> Result<()> {
    let keymap = Keymap::load(path);
    let report = ListReport {
        enabled: keybindings_enabled(),
        path: path.to_string_lossy().into_owned(),
        bindings: keymap
            .bindings
            .iter()
            .filter(|b| context.is_none_or(|c| c == b.context))
            .collect(),
        warnings: &keymap.warnings,
    };
    if let OutputFormat::Json = format {
        return print_json(&report);
    }

    note_if_disabled_for("This is the keymap they would use.");
    for context in BindingContext::ALL {
        let rows: Vec<_> = report
            .bindings
            .iter()
            .filter(|b| b.context == context)
            .collect();
        if rows.is_empty() {
            continue;
        }
        println!("[{}]", context.name());
        let width = rows.iter().map(|b| b.display.len()).max().unwrap_or(0);
        for b in rows {
            let action = b.action.map(KeyAction::name).unwrap_or("none (unbound)");
            let origin = match b.source {
                BindingSource::Default => "",
                BindingSource::User => "  (keybindings.toml)",
            };
            println!("  {:<width$}  {action}{origin}", b.display);
        }
        println!();
    }
    print_warnings_block(&keymap.warnings);
    Ok(())
}

#[derive(Debug, Serialize)]
struct ActionInfo {
    name: &'static str,
    group: ActionGroup,
    description: &'static str,
    default_keys: ContextKeys,
    keys: ContextKeys,
    /// Whether keybindings.toml changes this action's keys.
    customized: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ContextKeys {
    global: Vec<String>,
    overlay: Vec<String>,
}

fn action_info(keymap: &Keymap, action: KeyAction) -> ActionInfo {
    let keys_in = |c: BindingContext| -> Vec<String> {
        keymap
            .keys_for(action, c)
            .into_iter()
            .map(str::to_string)
            .collect()
    };
    let default_keys = ContextKeys {
        global: Keymap::default_keys(action, BindingContext::Global),
        overlay: Keymap::default_keys(action, BindingContext::Overlay),
    };
    let keys = ContextKeys {
        global: keys_in(BindingContext::Global),
        overlay: keys_in(BindingContext::Overlay),
    };
    ActionInfo {
        name: action.name(),
        group: action.group(),
        description: action.description(),
        customized: default_keys != keys,
        default_keys,
        keys,
    }
}

fn run_describe(path: &Path, target: Option<&str>, format: OutputFormat) -> Result<()> {
    let keymap = Keymap::load(path);
    let Some(target) = target else {
        let infos: Vec<ActionInfo> = KeyAction::all().map(|a| action_info(&keymap, a)).collect();
        if let OutputFormat::Json = format {
            return print_json(&infos);
        }
        describe_all(&infos);
        return Ok(());
    };

    if let Some(action) = KeyAction::from_name(target) {
        let info = action_info(&keymap, action);
        if let OutputFormat::Json = format {
            return print_json(&info);
        }
        describe_action(&info);
        return Ok(());
    }
    // Not an action: maybe a keystroke. Otherwise explain the action typo,
    // which is the likelier intent for a word.
    match parse_keys(target) {
        Ok(_) => run_get(path, target, BindingContext::Global, format),
        Err(_) => match parse_action(target) {
            Err(e) => bail!("`{target}` is neither an action nor a keystroke: {e}"),
            Ok(_) => bail!("`{target}` is not an action"),
        },
    }
}

fn describe_all(infos: &[ActionInfo]) {
    note_if_disabled_for("");
    println!("Actions — bind with `aura keys set <keys> <action>`:\n");
    let name_w = infos.iter().map(|i| i.name.len()).max().unwrap_or(0);
    let keys_of = |i: &ActionInfo| {
        let mut all = i.keys.global.clone();
        for k in &i.keys.overlay {
            all.push(format!("{k} (overlay)"));
        }
        if all.is_empty() {
            "—".to_string()
        } else {
            all.join(", ")
        }
    };
    let keys_w = infos
        .iter()
        .map(|i| keys_of(i).chars().count())
        .max()
        .unwrap_or(0)
        .min(28);
    for group in ActionGroup::ALL {
        println!("{}", group.label());
        for i in infos.iter().filter(|i| i.group == group) {
            let mark = if i.customized { "*" } else { " " };
            println!(
                "  {mark}{:<name_w$}  {:<keys_w$}  {}",
                i.name,
                keys_of(i),
                i.description
            );
        }
        println!();
    }
    if infos.iter().any(|i| i.customized) {
        println!("* keys changed in keybindings.toml\n");
    }
    println!("Explain one with `aura keys describe <action>` or `aura keys describe <keys>`.");
}

fn describe_action(info: &ActionInfo) {
    let fmt = |keys: &[String]| {
        if keys.is_empty() {
            "(unbound)".to_string()
        } else {
            keys.join(", ")
        }
    };
    println!("{}  ({})", info.name, info.group.label());
    println!("  {}", info.description);
    println!();
    println!("  default:  {}", fmt(&info.default_keys.global));
    println!("  current:  {}", fmt(&info.keys.global));
    if !info.default_keys.overlay.is_empty() || !info.keys.overlay.is_empty() {
        println!(
            "  overlay:  {}   (default: {})",
            fmt(&info.keys.overlay),
            fmt(&info.default_keys.overlay)
        );
    }
    println!();
    println!("  bind:     aura keys set <keys> {}", info.name);
    if info.customized {
        println!("  restore:  aura keys reset --action {}", info.name);
    }
}

#[derive(Debug, Serialize)]
struct Lookup {
    context: BindingContext,
    keys: String,
    display: String,
    /// `None` when nothing is bound (or it is explicitly unbound).
    action: Option<KeyAction>,
    source: Option<BindingSource>,
    /// True when an overlay lookup fell through to the global binding.
    inherited: bool,
}

fn run_get(path: &Path, keys: &str, context: BindingContext, format: OutputFormat) -> Result<()> {
    let parsed = parse_keys(keys).map_err(anyhow::Error::msg)?;
    let keymap = Keymap::load(path);
    let found = keymap.lookup(context, &parsed.canonical);
    let lookup = Lookup {
        context,
        keys: parsed.canonical.clone(),
        display: parsed.display.clone(),
        action: found.and_then(|b| b.action),
        source: found.map(|b| b.source),
        inherited: found.is_some_and(|b| b.context != context),
    };
    if let OutputFormat::Json = format {
        return print_json(&lookup);
    }
    let what = match found {
        None => "(unbound)".to_string(),
        Some(b) => {
            let action = b.action.map(KeyAction::name).unwrap_or("(unbound)");
            let source = match b.source {
                BindingSource::Default => "default",
                BindingSource::User => "keybindings.toml",
            };
            let via = if lookup.inherited {
                ", from [global]"
            } else {
                ""
            };
            format!("{action}   ({source}{via})")
        }
    };
    println!("[{}] {} → {what}", context.name(), parsed.display);
    if let Some(a) = lookup.action {
        println!("  {}", a.description());
    }
    Ok(())
}

fn run_validate(path: &Path, format: OutputFormat) -> Result<()> {
    let keymap = Keymap::load(path);
    match format {
        OutputFormat::Json => print_json(&keymap.warnings)?,
        OutputFormat::Text => {
            if !path.exists() {
                println!("{} does not exist; the defaults apply.", path.display());
            } else if keymap.warnings.is_empty() {
                println!("{}: OK", path.display());
            } else {
                println!("{}:", path.display());
                for w in &keymap.warnings {
                    println!("  warning: {w}");
                }
            }
        }
    }
    if !keymap.warnings.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

// ── write ────────────────────────────────────────────────────────────────────

fn run_set(path: &Path, keys: &str, action: &str, context: BindingContext) -> Result<()> {
    let action = parse_action(action).map_err(anyhow::Error::msg)?;
    let parsed = parse_keys(keys).map_err(anyhow::Error::msg)?;
    let mut file = KeymapFile::load(path)?;
    let before = file.keymap();
    let previous = before
        .lookup(context, &parsed.canonical)
        .and_then(|b| b.action);

    file.bind(context, keys, action)
        .map_err(anyhow::Error::msg)?;
    save(&file, path, &before)?;

    let was = match previous {
        Some(p) if Some(p) != action => format!("   (was {p})"),
        _ => String::new(),
    };
    match action {
        Some(a) => println!(
            "set [{}] \"{}\" = \"{a}\"{was}",
            context.name(),
            parsed.display
        ),
        None => println!("unbound [{}] \"{}\"{was}", context.name(), parsed.display),
    }
    Ok(())
}

fn run_reset(
    path: &Path,
    keys: Option<String>,
    action: Option<String>,
    all: bool,
    context: Option<BindingContext>,
) -> Result<()> {
    if !path.exists() {
        println!(
            "{} does not exist; the defaults already apply.",
            path.display()
        );
        return Ok(());
    }
    let mut file = KeymapFile::load(path)?;
    let before = file.keymap();

    let removed = if all {
        file.clear(context)
    } else if let Some(name) = action {
        let Some(action) = parse_action(&name).map_err(anyhow::Error::msg)? else {
            bail!("`--action none` is not an action");
        };
        file.restore_action(context.unwrap_or(BindingContext::Global), action)
    } else if let Some(keys) = keys {
        file.remove(context.unwrap_or(BindingContext::Global), &keys)
            .map_err(anyhow::Error::msg)?
    } else {
        bail!("say what to reset: a keystroke, `--action <name>`, or `--all`");
    };

    if removed == 0 {
        println!("Nothing to reset: keybindings.toml has no matching entry.");
        return Ok(());
    }
    save(&file, path, &before)?;
    println!(
        "Removed {removed} entr{} from {}; the defaults apply there again.",
        if removed == 1 { "y" } else { "ies" },
        path.display()
    );
    Ok(())
}

fn run_wizard(path: &Path, context: BindingContext) -> Result<()> {
    let mut file = KeymapFile::load(path)?;
    let before = file.keymap();

    println!("Editing {} — [{}]", path.display(), context.name());
    note_if_disabled_for("Changes apply once they are turned on.");
    println!(
        "For each action: Enter keeps its keys; type keys separated by commas to replace them\n\
         (e.g. `j, ctrl-n`); `none` unbinds it; `default` restores its defaults; `stop` ends early.\n"
    );

    let stdin = io::stdin();
    let mut changed = false;
    'groups: for group in ActionGroup::ALL {
        println!("── {} ──", group.label());
        for action in KeyAction::all().filter(|a| a.group() == group) {
            loop {
                let map = file.keymap();
                let current = map.keys_for(action, context);
                let custom = current.iter().map(|k| k.to_string()).collect::<Vec<_>>()
                    != Keymap::default_keys(action, context);
                print!(
                    "{}{} — {} [{}]: ",
                    if custom { "*" } else { "" },
                    action.name(),
                    action.description(),
                    if current.is_empty() {
                        "unbound".to_string()
                    } else {
                        current.join(", ")
                    }
                );
                io::stdout().flush().ok();

                let mut line = String::new();
                if stdin.lock().read_line(&mut line)? == 0 {
                    // EOF: keep what we have.
                    println!();
                    break 'groups;
                }
                let input = line.trim();
                let result = match input {
                    "" => break,
                    "stop" => break 'groups,
                    "default" => {
                        file.restore_action(context, action);
                        Ok(Vec::new())
                    }
                    "none" => file.set_action_keys(context, action, &[]),
                    list => {
                        let keys: Vec<String> = list
                            .split(',')
                            .map(str::trim)
                            .filter(|k| !k.is_empty())
                            .map(str::to_string)
                            .collect();
                        file.set_action_keys(context, action, &keys)
                    }
                };
                match result {
                    Ok(notes) => {
                        for n in notes {
                            println!("  note: {n}");
                        }
                        changed = true;
                        break;
                    }
                    Err(e) => println!("  {e}\n"),
                }
            }
        }
        println!();
    }

    if changed {
        save(&file, path, &before)?;
        println!("Saved {}", path.display());
    } else {
        println!("No changes.");
    }
    Ok(())
}

fn run_merge(
    path: &Path,
    source: &Path,
    prefer: PreferArg,
    check: bool,
    format: OutputFormat,
) -> Result<()> {
    let content = if source == Path::new("-") {
        let mut s = String::new();
        io::stdin().read_to_string(&mut s).context("read stdin")?;
        s
    } else {
        std::fs::read_to_string(source).with_context(|| format!("read {}", source.display()))?
    };
    let theirs =
        KeymapFile::parse(&content).with_context(|| format!("parse {}", source.display()))?;
    let prefer = match prefer {
        PreferArg::Theirs => MergePrefer::Theirs,
        PreferArg::Ours => MergePrefer::Ours,
    };

    let mut file = KeymapFile::load(path)?;
    let before = file.keymap();
    let report = file.merge(&theirs, prefer);
    let changes = report.changes_anything(prefer);

    match format {
        OutputFormat::Json => print_json(&report)?,
        OutputFormat::Text => print_merge(&report, source, path, prefer),
    }

    if check {
        if changes {
            if let OutputFormat::Text = format {
                println!(
                    "\nWould change {} — run without --check to apply.",
                    path.display()
                );
            }
            std::process::exit(1);
        }
        return Ok(());
    }
    if changes {
        save(&file, path, &before)?;
        if let OutputFormat::Text = format {
            println!("\nSaved {}", path.display());
        }
    } else if let OutputFormat::Text = format {
        println!("\nNothing to merge.");
    }
    Ok(())
}

fn print_merge(report: &MergeReport, source: &Path, path: &Path, prefer: MergePrefer) {
    let side = match prefer {
        MergePrefer::Theirs => "theirs",
        MergePrefer::Ours => "ours",
    };
    println!(
        "Merging {} into {} (conflicts: {side} win)",
        source.display(),
        path.display()
    );
    for c in &report.added {
        println!(
            "  + [{}] \"{}\" = \"{}\"",
            c.context.name(),
            c.keys,
            c.theirs
        );
    }
    for c in &report.changed {
        println!(
            "  ~ [{}] \"{}\": {} → {}",
            c.context.name(),
            c.keys,
            c.ours.as_deref().unwrap_or("—"),
            c.theirs
        );
    }
    for c in &report.kept {
        println!(
            "  ! [{}] \"{}\": kept {} (theirs: {})",
            c.context.name(),
            c.keys,
            c.ours.as_deref().unwrap_or("—"),
            c.theirs
        );
    }
    if let Some((ours, theirs)) = report.use_defaults {
        match prefer {
            MergePrefer::Theirs => println!("  ~ use_defaults: {ours} → {theirs}"),
            MergePrefer::Ours => println!("  ! use_defaults: kept {ours} (theirs: {theirs})"),
        }
    }
    if report.unchanged > 0 {
        println!("  = {} already the same", report.unchanged);
    }
    if !report.skipped.is_empty() {
        println!("\nNot merged — problems in {}:", source.display());
        for s in &report.skipped {
            println!("  {s}");
        }
    }
}

fn run_init(path: &Path, force: bool, full: bool) -> Result<()> {
    if path.exists() && !force {
        println!(
            "{} already exists (pass --force to overwrite).",
            path.display()
        );
        return Ok(());
    }
    let contents = if full {
        Keymap::defaults().to_explicit_toml()
    } else {
        Keymap::default_file_contents()
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create config dir {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}

fn run_document(path: &Path, force: bool) -> Result<()> {
    let existed = path.exists();
    let file = KeymapFile::load(path)?;
    let (documented, dropped) = file.document();
    if !dropped.is_empty() {
        println!("These can't be carried over and would be dropped:");
        for d in &dropped {
            println!("  {d}");
        }
        if !force {
            println!("\nFix them first, or pass --force to drop them.");
            std::process::exit(1);
        }
        println!();
    }
    documented.save(path)?;
    if existed {
        println!(
            "Rewrote {} with inline documentation ({} entr{} kept).",
            path.display(),
            documented.entries().len(),
            if documented.entries().len() == 1 {
                "y"
            } else {
                "ies"
            }
        );
    } else {
        println!("Created {} with inline documentation.", path.display());
    }
    Ok(())
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Save `file`, then point out any warning the change introduced.
fn save(file: &KeymapFile, path: &Path, before: &Keymap) -> Result<()> {
    file.save(path)?;
    let after = file.keymap();
    for w in after
        .warnings
        .iter()
        .filter(|w| !before.warnings.contains(w))
    {
        eprintln!("warning: {w}");
    }
    Ok(())
}

/// `[keybindings] enabled`, defaulting to on when config.toml can't be read
/// (the modal would fall back the same way).
fn keybindings_enabled() -> bool {
    let path = AppConfig::default_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| AppConfig::parse(&c).ok())
        .map(|(cfg, _)| cfg.keybindings.enabled)
        .unwrap_or(true)
}

fn note_if_disabled_for(extra: &str) {
    if !keybindings_enabled() {
        println!(
            "note: keyboard shortcuts are off ([keybindings] enabled = false in config.toml). {extra}\n"
        );
    }
}

fn print_warnings_block(warnings: &[KeymapWarning]) {
    if warnings.is_empty() {
        return;
    }
    println!("Warnings:");
    for w in warnings {
        println!("  {w}");
    }
}

fn print_warnings_stderr(warnings: &[KeymapWarning]) {
    for w in warnings {
        eprintln!("warning: {w}");
    }
}
