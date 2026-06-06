use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

use aura_core::{
    activity::{ActivityMonitor, ClaudeSession, ProcView},
    config::{AgentConfig, AgentKind, AppConfig, PluginConfig},
    lexicon::{self, Lexicon},
    net::{fleet::FleetRow, pairing::PairingSecret, secret_store},
    plugin::{PluginContent, PluginPanel, PluginRunner, PluginSection},
    quota::{
        forecast, pacing, CodexQuota, ForecastSnapshot, ForecastStatus, ForecastWindow,
        GeminiQuota, PacingStatus, QuotaApi, QuotaSnapshot, QuotaSource, QuotaWindow, SessionBudget,
    },
    reader::{
        make_reader, CacheEfficiency, InsightsSnapshot, Period, ProjectStat, SessionInsight,
        UsageSnapshot,
    },
    state::AppState,
    theme::Theme,
};
use chrono::{DateTime, Local, Timelike, Utc};
use gpui::{
    div, prelude::*, px, rgb, size, svg, Animation, AnimationExt, AnyElement, ClickEvent, Context,
    Pixels, ScrollHandle, SharedString, Window,
};

use crate::format::{
    compact_tokens, duration, hour_of_day, locale_uses_12h, system_locale, thousands,
};
use crate::updater::{self, UpdateInfo};

/// Fixed window width. The window grows vertically to fit content (see
/// `on_children_prepainted` in `render`), so only the height is dynamic.
/// Aliased to the single source of truth in `placement` so the open-time
/// bounds and the auto-fit resize can never disagree on width.
const WINDOW_WIDTH: f32 = crate::placement::MODAL_W;

/// Braille spinner frames. Matches the `cli-spinners` "dots" preset
/// (see `.design/loading.md`). 10 frames, advanced every 80ms while
/// `is_loading == true`.
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_TICK_MS: u64 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Agent,
    Plugin,
}

/// Section identifiers used by the agent (Claude Code) view. Plugin
/// sections are addressed by their `id` string instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentSection {
    Quota,
    Forecast,
    Summary,
    Models,
    Insights,
    /// Cross-machine usage comparison. Only present when `[fleet].enabled`.
    Fleet,
    /// Live Claude Code process monitor. Only present when `[activity].enabled`.
    Activity,
}

impl AgentSection {
    fn label(self, lex: &Lexicon) -> &'static str {
        match self {
            Self::Quota => lex.tab_quota,
            Self::Forecast => lex.tab_forecast,
            Self::Summary => lex.tab_summary,
            Self::Models => lex.tab_models,
            Self::Insights => lex.tab_insights,
            Self::Fleet => "Fleet",
            Self::Activity => "Activity",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Forecast => "forecast",
            Self::Summary => "summary",
            Self::Models => "models",
            Self::Insights => "insights",
            Self::Fleet => "fleet",
            Self::Activity => "activity",
        }
    }

    /// Whether this section filters data by the active period. Quota
    /// reports rolling 5h / 7d subscription windows fixed by the API, so
    /// the period pills don't apply. Fleet is account-wide and live, and
    /// Activity is a live process monitor, so both ignore the period too.
    fn uses_period(self) -> bool {
        match self {
            Self::Quota | Self::Forecast | Self::Fleet | Self::Activity => false,
            Self::Summary | Self::Models | Self::Insights => true,
        }
    }
}

pub struct AuraView {
    config: AppConfig,
    config_path: PathBuf,
    theme: Theme,
    theme_path: PathBuf,
    state: AppState,
    active_profile: String,
    active_period: Period,

    /// Whether we're showing an agent (Claude Code etc.) or a plugin.
    mode: Mode,
    /// When `mode == Mode::Plugin`, which plugin (by name) is selected.
    active_plugin: Option<String>,

    /// Active section within the agent's view.
    active_agent_section: AgentSection,
    /// Active section within the plugin's view (by section id).
    active_plugin_section: Option<String>,

    snapshot: Option<UsageSnapshot>,
    quota: Option<QuotaSnapshot>,
    /// Token caps for the session-budget gauge, derived during refresh from the
    /// quota windows + a local JSONL token sum. `None` when pacing is off or
    /// usage is too thin to invert a reliable cap.
    pacing_caps: Option<pacing::Caps>,
    forecast: Option<ForecastSnapshot>,
    /// Indexed by plugin name.
    plugin_panels: Vec<(String, PluginPanel)>,

    /// Latest GitHub release info, populated by a background fetch at
    /// startup. `None` means the check hasn't completed yet, failed, or the
    /// remote version is not newer than the local build. Stays set after
    /// success — dismissing only writes the version into config; the field
    /// itself isn't cleared so re-opening the modal in the same session
    /// honours the dismissal without a flicker.
    update: Option<UpdateInfo>,

    show_more_modal: bool,
    show_settings_panel: bool,
    is_loading: bool,
    /// Index into `SPINNER_FRAMES`, advanced by a timer while `is_loading`.
    spinner_frame: usize,
    error: Option<String>,
    /// Height we last asked the window to be. Read by the
    /// `on_children_prepainted` callback so we only issue a `Window::resize`
    /// when the measured content height actually changes.
    last_window_height: Rc<Cell<Pixels>>,
    /// Tracks the body's scroll state so the window-resize callback can read
    /// `max_offset` to recover the body's natural content height (the body is
    /// allowed to shrink below its content when the window hits the screen
    /// cap; without this we couldn't tell capped layouts from natural ones).
    body_scroll: ScrollHandle,
    /// On Windows the modal is DWM-cloaked on open to hide the first-frame
    /// resize (MODAL_H → content height). This flag drives the uncloak that
    /// fires in `on_next_frame` after the first resize, so the window becomes
    /// visible at the correct size with no visible flash.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    needs_uncloak: Rc<Cell<bool>>,

    /// Pairing code shown transiently in the Fleet panel right after the user
    /// clicks "Pair a machine" / "Show code". Cleared on panel close so it is
    /// never persisted to the visible UI (screenshot-leak mitigation).
    fleet_code: Option<String>,
    /// One-line status / error for the Fleet panel ("Joined fleet", "Clipboard
    /// empty", a decode error, etc.).
    fleet_status: Option<String>,

    /// Live Claude Code process monitor. Owns a long-lived `sysinfo::System`
    /// so CPU% deltas compute correctly across ticks. Lazily created the first
    /// time the Activity tab is sampled and dropped with the view when the
    /// modal closes, so it costs nothing until used.
    activity_monitor: Option<ActivityMonitor>,
    /// The most recent Activity sample, rendered by `render_activity`.
    activity_sessions: Vec<ClaudeSession>,
    /// True once the monitor has a CPU baseline (after the first sample). Until
    /// then the UI shows "measuring…" instead of a bogus 0% reading.
    activity_primed: bool,
    /// Monotonic token bumped each time the live Activity loop starts. The
    /// self-rescheduling tick captures its generation and stops as soon as a
    /// newer loop supersedes it (e.g. the user leaves and re-enters the tab),
    /// guaranteeing exactly one live sampler at a time.
    activity_tick: u64,
}

/// Bundle of results produced by the background refresh task. Errors that
/// previously landed on `self.error` are bubbled up here so they survive
/// the trip across threads.
struct RefreshResult {
    /// Reloaded config — `None` means "keep the old one".
    config: Option<AppConfig>,
    /// Reloaded theme — `None` means "keep the old one".
    theme: Option<Theme>,
    /// `Some` if the active profile had to fall back to the first agent.
    fallback_profile: Option<String>,
    snapshot: Option<UsageSnapshot>,
    quota: Option<QuotaSnapshot>,
    pacing_caps: Option<pacing::Caps>,
    forecast: Option<ForecastSnapshot>,
    plugin_panels: Vec<(String, PluginPanel)>,
    error: Option<String>,
}

impl AuraView {
    pub fn new(
        config: AppConfig,
        config_path: PathBuf,
        state: AppState,
        cx: &mut Context<Self>,
    ) -> Self {
        let active_profile = state
            .active_profile
            .clone()
            .or_else(|| config.agents.first().map(|a| a.name.clone()))
            .unwrap_or_else(|| "(none)".to_string());

        let active_period = match config.display.default_period.as_str() {
            "7d" => Period::Last7Days,
            "30d" => Period::Last30Days,
            _ => Period::AllTime,
        };

        let theme_path = Theme::default_path();
        let theme = Theme::load(&theme_path).unwrap_or_else(|e| {
            eprintln!("aura: theme.toml load failed ({e}); using defaults");
            Theme::default()
        });

        let mut view = Self {
            config,
            config_path,
            theme,
            theme_path,
            state,
            active_profile,
            active_period,
            mode: Mode::Agent,
            active_plugin: None,
            active_agent_section: AgentSection::Quota,
            active_plugin_section: None,
            snapshot: None,
            quota: None,
            pacing_caps: None,
            forecast: None,
            plugin_panels: Vec::new(),
            update: None,
            show_more_modal: false,
            show_settings_panel: false,
            is_loading: false,
            spinner_frame: 0,
            error: None,
            last_window_height: Rc::new(Cell::new(Pixels::ZERO)),
            body_scroll: ScrollHandle::new(),
            needs_uncloak: Rc::new(Cell::new(cfg!(target_os = "windows"))),
            fleet_code: None,
            fleet_status: None,
            activity_monitor: None,
            activity_sessions: Vec::new(),
            activity_primed: false,
            activity_tick: 0,
        };
        // Fleet runs at the process level (see `main::FleetManager`), not here —
        // so it publishes/polls with the modal closed. The view only reads peers
        // for rendering and signals the manager via `runtime::mark_fleet_dirty`.
        //
        // Initial load: kick off the async refresh now so the spinner can
        // render on first paint instead of blocking construction.
        view.refresh(cx);
        // Fire-and-forget release check. Skipped when the user disabled
        // update prompts via `[update] dismiss_all = true` — that's the
        // kill switch documented on `UpdateConfig::dismiss_all`.
        if !view.config.update.dismiss_all {
            view.spawn_update_check(cx);
        }
        view
    }

    /// Spawn the GitHub release check on the background executor and push
    /// the result back onto `AuraView.update` via `cx.notify()`. Errors are
    /// logged to stderr and otherwise ignored — a failed check leaves the
    /// header unchanged (see plan: degraded experience > noisy banner).
    fn spawn_update_check(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { updater::fetch_latest() })
                .await;
            match result {
                Ok(Some(info)) => {
                    let _ = this.update(cx, |view, cx| {
                        view.update = Some(info);
                        cx.notify();
                    });
                }
                Ok(None) => { /* already up-to-date */ }
                Err(e) => eprintln!("aura: update check failed: {e}"),
            }
        })
        .detach();
    }

    /// Kick off an async refresh. Returns immediately — the heavy work
    /// (snapshot scan, quota API call, plugin subprocesses) runs on the
    /// background executor. The result is applied via `apply_refresh_result`
    /// back on the foreground thread.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.is_loading {
            // A refresh is already in flight; don't double-spawn.
            return;
        }
        self.is_loading = true;
        self.error = None;
        cx.notify();
        self.spawn_spinner_tick(cx);

        let config_path = self.config_path.clone();
        let theme_path = self.theme_path.clone();
        let active_profile = self.active_profile.clone();
        let period = self.active_period;

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { do_refresh(config_path, theme_path, active_profile, period) })
                .await;

            this.update(cx, |view, cx| {
                view.apply_refresh_result(result, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Apply a `RefreshResult` back to the view on the foreground thread.
    fn apply_refresh_result(&mut self, result: RefreshResult, cx: &mut Context<Self>) {
        if let Some(cfg) = result.config {
            // Push shared-config fields out to the tray loop so its
            // focus-loss check picks up the new dismiss_on_focus_loss
            // value on the next poll without a service restart.
            crate::runtime::set_from_config(&cfg);
            // If the user toggled `[fleet]` (enable/disable/broker/label) since
            // the last refresh, signal the process-level Fleet manager to
            // reconcile so it (re)starts or stops the sync with the new params.
            let fleet_changed = self.config.fleet != cfg.fleet;
            self.config = cfg;
            if fleet_changed {
                crate::runtime::mark_fleet_dirty();
            }
        }
        if let Some(theme) = result.theme {
            self.theme = theme;
        }
        if let Some(fallback) = result.fallback_profile {
            self.active_profile = fallback;
        }
        self.snapshot = result.snapshot;
        self.quota = result.quota;
        self.pacing_caps = result.pacing_caps;
        self.forecast = result.forecast;
        self.plugin_panels = result.plugin_panels;
        self.error = result.error;

        // Initialize the active plugin selection if absent or stale.
        if self.active_plugin.is_none() {
            self.active_plugin = self.plugin_panels.first().map(|(name, _)| name.clone());
        }
        // Initialize the plugin section if absent or no longer present.
        let known_section = self
            .active_plugin
            .as_deref()
            .and_then(|name| self.plugin_panels.iter().find(|(n, _)| n == name))
            .and_then(|(_, panel)| {
                self.active_plugin_section
                    .as_deref()
                    .and_then(|id| panel.section(id))
                    .map(|s| s.id.clone())
            });
        if known_section.is_none() {
            self.active_plugin_section = self
                .active_plugin
                .as_deref()
                .and_then(|name| self.plugin_panels.iter().find(|(n, _)| n == name))
                .and_then(|(_, panel)| panel.sections.first().map(|s| s.id.clone()));
        }

        self.is_loading = false;
        cx.notify();
    }

    /// Schedule a single spinner-frame tick. While `is_loading` remains true
    /// the tick re-schedules itself; once loading ends the chain stops. This
    /// gives us animation without needing a continuous animation driver.
    fn spawn_spinner_tick(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SPINNER_TICK_MS))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.is_loading {
                    view.spinner_frame = (view.spinner_frame + 1) % SPINNER_FRAMES.len();
                    cx.notify();
                    view.spawn_spinner_tick(cx);
                }
            });
        })
        .detach();
    }
}

/// Pure background-thread refresh worker. All the heavy I/O (config reload,
/// snapshot scan, quota API call, plugin subprocesses) happens here so the UI
/// thread can keep rendering the spinner. Errors are bundled into the
/// returned `RefreshResult` rather than returned via `Result` so partial
/// success (e.g. quota OK but snapshot failed) still surfaces.
fn do_refresh(
    config_path: PathBuf,
    theme_path: PathBuf,
    active_profile: String,
    period: Period,
) -> RefreshResult {
    // Reload theme alongside the config so an edit to either file takes
    // effect on the same "Refresh" click (see .design/customization.md
    // §"Hot reload"). Failures fall back to `Theme::default()` rather than
    // failing the whole refresh — a malformed theme.toml shouldn't blank
    // the modal.
    let theme = Some(Theme::load(&theme_path).unwrap_or_else(|e| {
        eprintln!("aura: theme.toml reload failed ({e}); using defaults");
        Theme::default()
    }));

    // Reload config so edits made via the settings button take effect.
    // `load_with_discovery` also picks up any binaries added to the user
    // plugins dir since the last open (via `aura plugin add` or a manual
    // drop-in), keeping the modal's plugin list in sync without a restart.
    let config = match AppConfig::load_with_discovery(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return RefreshResult {
                config: None,
                theme,
                fallback_profile: None,
                snapshot: None,
                quota: None,
                forecast: None,
                pacing_caps: None,
                plugin_panels: Vec::new(),
                error: Some(format!("Could not reload config: {e}")),
            };
        }
    };

    // If the active profile vanished from the reloaded config, fall back to
    // the first agent. We capture this so the foreground thread can apply it.
    let (resolved_profile, fallback_profile) =
        if config.agents.iter().any(|a| a.name == active_profile) {
            (active_profile.clone(), None)
        } else if let Some(first) = config.agents.first() {
            (first.name.clone(), Some(first.name.clone()))
        } else {
            (active_profile.clone(), None)
        };

    let Some(agent) = config
        .agents
        .iter()
        .find(|a| a.name == resolved_profile)
        .cloned()
    else {
        return RefreshResult {
            config: Some(config),
            theme,
            fallback_profile,
            snapshot: None,
            quota: None,
            pacing_caps: None,
            forecast: None,
            plugin_panels: Vec::new(),
            error: Some(format!("Profile `{resolved_profile}` not found in config")),
        };
    };

    let mut error: Option<String> = None;

    let snapshot = match make_reader(&agent).snapshot(period) {
        Ok(snap) => Some(snap),
        Err(e) => {
            error = Some(format!("Snapshot failed: {e}"));
            None
        }
    };

    // Quota windows: per-agent source → unavailable, never `Err`.
    let agent_path = agent.resolved_config_path();
    let mut quota = Some(match agent.kind {
        AgentKind::ClaudeCode => QuotaApi::new(agent_path.clone()).snapshot(),
        AgentKind::Codex => CodexQuota::new(agent_path.clone()).snapshot(),
        AgentKind::Gemini => GeminiQuota::new(agent_path.clone()).snapshot(),
    });

    // Budget pacing (F2, Claude Code only): when enabled and we have live API
    // percentages, learn the active-session pattern from a local JSONL scan and
    // attach it to the snapshot so `pacing::session_budget` can pace on it. This
    // rides the existing refresh — no new timer/thread.
    let mut pacing_caps: Option<pacing::Caps> = None;
    if config.pacing.enabled && agent.kind == AgentKind::ClaudeCode {
        if let Some(q) = quota.as_mut() {
            if q.source == QuotaSource::Api {
                let now = Utc::now();
                let sessions = pacing::collect_session_tokens(
                    &agent_path,
                    now,
                    config.pacing.history_days,
                );
                // The 5h "session" window's `resets_at` anchors the 5h grid the
                // pattern buckets history onto.
                let session_resets_at = q
                    .windows
                    .iter()
                    .find(|w| w.label == "Current session")
                    .and_then(|w| w.resets_at)
                    .unwrap_or(now);
                q.pacing_pattern = Some(pacing::learn_pattern(
                    &sessions,
                    now,
                    config.pacing.history_days,
                    config.pacing.active_session_min_tokens,
                    session_resets_at,
                ));
                // Token caps need both the quota windows and the JSONL token
                // sums — compute them here where both are in scope. `None` when
                // the windows are missing or usage is too thin to invert a cap.
                if let (Some(weekly), Some(session)) = (
                    q.windows
                        .iter()
                        .find(|w| w.label == "Current week (all models)")
                        .cloned(),
                    q.windows
                        .iter()
                        .find(|w| w.label == "Current session")
                        .cloned(),
                ) {
                    pacing_caps =
                        pacing::compute_caps(&agent_path, &weekly, &session, now).ok();
                }
            }
        }
    }

    // Forecast piggybacks on the just-loaded quota snapshot. Same refresh
    // cadence, no extra I/O.
    let forecast = quota.as_ref().map(|q| forecast::forecast(q, Utc::now()));

    // Plugins: each runs as a subprocess with `--period` passed through.
    let plugin_panels = config
        .plugins
        .iter()
        .map(|p| (p.name.clone(), PluginRunner::run_with_period(p, period)))
        .collect();

    RefreshResult {
        config: Some(config),
        theme,
        fallback_profile,
        snapshot,
        quota,
        pacing_caps,
        forecast,
        plugin_panels,
        error,
    }
}

impl AuraView {
    /// Open the config file in the desktop's default editor.
    fn open_config(&mut self, cx: &mut Context<Self>) {
        // Ensure the file exists so the editor doesn't open a blank buffer.
        if !self.config_path.exists() {
            if let Err(e) = AppConfig::load(&self.config_path) {
                self.error = Some(format!("Could not create config: {e}"));
                cx.notify();
                return;
            }
        }
        self.open_in_editor(&self.config_path.clone(), cx);
    }

    /// Open `theme.toml` in the user's editor. Mirrors `open_config`. The
    /// file is seeded from the built-in default theme on first click so the
    /// editor opens with an editable example rather than a blank buffer.
    fn open_theme(&mut self, cx: &mut Context<Self>) {
        if !self.theme_path.exists() {
            if let Some(parent) = self.theme_path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    self.error = Some(format!("Could not create theme dir: {e}"));
                    cx.notify();
                    return;
                }
            }
            if let Err(e) = std::fs::write(&self.theme_path, Theme::DEFAULT_TOML) {
                self.error = Some(format!("Could not create theme.toml: {e}"));
                cx.notify();
                return;
            }
        }
        self.open_in_editor(&self.theme_path.clone(), cx);
    }

    fn open_in_editor(&mut self, path: &std::path::Path, _cx: &mut Context<Self>) {
        // `platform::open_path` invokes the OS default handler (xdg-open /
        // open / ShellExecuteW) and, on failure, falls back to revealing the
        // file in the system file manager. We never block the UI on the
        // spawn — both the open and the fallback happen on a detached thread.
        crate::platform::open_path(path);
    }

    fn set_profile(&mut self, name: String, cx: &mut Context<Self>) {
        if self.active_profile != name {
            self.active_profile = name.clone();
            self.state.active_profile = Some(name);
            let _ = self.state.save();
            // Reset the spinner so a profile switch feels like a fresh start.
            self.spinner_frame = 0;
            self.refresh(cx);
        }
    }

    fn set_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if self.active_period != period {
            self.active_period = period;
            self.refresh(cx);
        }
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }

    fn set_agent_section(&mut self, section: AgentSection, cx: &mut Context<Self>) {
        if self.active_agent_section != section {
            self.active_agent_section = section;
            // Leaving the Fleet tab hides any transiently-shown pairing code so
            // it never lingers on screen.
            if section != AgentSection::Fleet {
                self.fleet_code = None;
            }
            // Bumping the generation token stops any running Activity sampler
            // (its captured generation no longer matches). Entering the tab
            // starts a fresh one; this guarantees a single live sampler and
            // zero background cost whenever the tab isn't on screen.
            self.activity_tick = self.activity_tick.wrapping_add(1);
            if section == AgentSection::Activity {
                self.start_activity_loop(cx);
            }
            cx.notify();
        }
    }

    // ── Activity (live Claude Code process monitor) ─────────────────────────────

    /// Begin (or restart) the live sampling loop for the Activity tab. Mirrors
    /// the spinner-tick pattern (`spawn_spinner_tick`): a self-rescheduling
    /// `cx.spawn` chain whose continuation is gated on a still-valid condition.
    /// Here the gate is "the captured generation still matches AND the tab is
    /// still Activity" — so the loop dies the instant the user switches tabs
    /// or the modal closes (the view drops and `update` fails). The first
    /// sample primes the CPU baseline; subsequent ones (spaced `refresh_secs`,
    /// itself ≥ `MINIMUM_CPU_UPDATE_INTERVAL`) carry real CPU deltas.
    fn start_activity_loop(&mut self, cx: &mut Context<Self>) {
        // Reuse the monitor across visits so its CPU baseline survives — a
        // re-entered tab then shows real CPU% immediately instead of
        // "measuring…". A first-ever visit creates it and pays the one-tick
        // priming cost.
        if self.activity_monitor.is_none() {
            self.activity_monitor = Some(ActivityMonitor::new());
        }
        let generation = self.activity_tick;
        self.sample_activity(cx);
        self.spawn_activity_tick(generation, cx);
    }

    /// Take one sample from the monitor into `activity_sessions` and mark the
    /// baseline primed once the monitor reports it has one.
    fn sample_activity(&mut self, cx: &mut Context<Self>) {
        if let Some(monitor) = self.activity_monitor.as_mut() {
            self.activity_sessions = monitor.sample();
            self.activity_primed = monitor.is_primed();
        }
        cx.notify();
    }

    /// Schedule a single live re-sample after `refresh_secs`. Re-schedules
    /// itself only while `generation` is still current and the Activity tab is
    /// still active; otherwise the chain stops (zero background cost).
    fn spawn_activity_tick(&self, generation: u64, cx: &mut Context<Self>) {
        let refresh_secs = self.config.activity.refresh_secs.max(1);
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(refresh_secs))
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.activity_tick == generation
                    && view.active_agent_section == AgentSection::Activity
                {
                    view.sample_activity(cx);
                    view.spawn_activity_tick(generation, cx);
                }
            });
        })
        .detach();
    }

    // ── Fleet ─────────────────────────────────────────────────────────────────
    //
    // The Fleet sync runs at the process level (see `main::FleetManager`), not
    // here — so it publishes/polls 24/7 with the modal closed. The view's only
    // jobs are to (a) read peers for rendering via `runtime::fleet_state()` and
    // (b) mutate the keychain secret on pair/join/leave, then signal the manager
    // to reconcile via `runtime::mark_fleet_dirty()`.

    /// "Pair a machine": generate a fresh secret, store it, signal the manager
    /// to (re)start the sync, and surface the code to copy onto the other
    /// machine.
    fn fleet_generate_code(&mut self, cx: &mut Context<Self>) {
        let secret = PairingSecret::generate();
        if let Err(e) = secret_store::set(&secret) {
            self.fleet_status = Some(format!("Could not store secret: {e}"));
            cx.notify();
            return;
        }
        let code = secret.to_code();
        // The process-level manager reconciles against the new secret on its
        // next poll tick.
        crate::runtime::mark_fleet_dirty();
        self.fleet_code = Some(code);
        self.fleet_status = Some("Code generated — copy it to the other machine.".to_string());
        cx.notify();
    }

    /// Copy the currently-shown pairing code to the clipboard.
    fn fleet_copy_code(&mut self, cx: &mut Context<Self>) {
        if let Some(code) = &self.fleet_code {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.clone()));
            self.fleet_status = Some("Copied to clipboard.".to_string());
            cx.notify();
        }
    }

    /// "Join fleet": read a pairing code from the clipboard, derive the secret,
    /// store it, and signal the manager to start syncing.
    fn fleet_join_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else {
            self.fleet_status = Some("Clipboard is empty — copy the code first.".to_string());
            cx.notify();
            return;
        };
        match PairingSecret::from_code(&text) {
            Ok(secret) => {
                if let Err(e) = secret_store::set(&secret) {
                    self.fleet_status = Some(format!("Could not store secret: {e}"));
                    cx.notify();
                    return;
                }
                crate::runtime::mark_fleet_dirty();
                self.fleet_code = None;
                self.fleet_status = Some("Joined fleet.".to_string());
            }
            Err(e) => {
                self.fleet_status = Some(format!("Invalid code: {e}"));
            }
        }
        cx.notify();
    }

    /// "Leave fleet": delete the secret from the keychain and signal the manager
    /// to stop syncing.
    fn fleet_leave(&mut self, cx: &mut Context<Self>) {
        match secret_store::delete() {
            Ok(()) => self.fleet_status = Some("Left fleet.".to_string()),
            Err(e) => self.fleet_status = Some(format!("Could not delete secret: {e}")),
        }
        crate::runtime::mark_fleet_dirty();
        self.fleet_code = None;
        cx.notify();
    }

    fn set_plugin(&mut self, name: String, cx: &mut Context<Self>) {
        if self.active_plugin.as_deref() != Some(name.as_str()) {
            self.active_plugin = Some(name);
            // Reset the active section to the new plugin's first section.
            self.active_plugin_section = self
                .current_plugin_panel()
                .and_then(|p| p.sections.first().map(|s| s.id.clone()));
            cx.notify();
        }
    }

    fn set_plugin_section(&mut self, id: String, cx: &mut Context<Self>) {
        if self.active_plugin_section.as_deref() != Some(id.as_str()) {
            self.active_plugin_section = Some(id);
            cx.notify();
        }
    }

    /// Whether the "Update available" header chip should render. Pure
    /// wrapper around [`should_show_update_button`] so the gating logic is
    /// unit-testable without spinning up a full `AuraView`.
    fn show_update_button(&self) -> bool {
        should_show_update_button(self.update.as_ref(), &self.config.update)
    }

    /// Open the README's `### Updating` anchor — the two-curl / two-iex
    /// flow lives there. We do not auto-download; see the plan's
    /// "Non-goals" for why.
    fn open_update_instructions(&mut self, _cx: &mut Context<Self>) {
        open_url(updater::UPDATE_INSTRUCTIONS_URL);
    }

    /// Persist the dismissal so the button stays hidden across relaunches
    /// for *this* version. A newer release than `info.latest` re-shows the
    /// button automatically because `show_update_button` compares strings.
    fn dismiss_update(&mut self, cx: &mut Context<Self>) {
        let Some(info) = &self.update else {
            return;
        };
        let version = info.latest.to_string();
        self.config.update.dismissed_version = Some(version);
        if let Err(e) = self.config.save(&self.config_path) {
            self.error = Some(format!("Could not save dismissal: {e}"));
        }
        cx.notify();
    }

    fn toggle_more_modal(&mut self, cx: &mut Context<Self>) {
        self.show_more_modal = !self.show_more_modal;
        cx.notify();
    }

    fn close_more_modal(&mut self, cx: &mut Context<Self>) {
        self.show_more_modal = false;
        cx.notify();
    }

    fn toggle_settings_panel(&mut self, cx: &mut Context<Self>) {
        self.show_settings_panel = !self.show_settings_panel;
        cx.notify();
    }

    fn close_settings_panel(&mut self, cx: &mut Context<Self>) {
        self.show_settings_panel = false;
        cx.notify();
    }

    fn current_agent(&self) -> Option<&AgentConfig> {
        self.config
            .agents
            .iter()
            .find(|a| a.name == self.active_profile)
    }

    /// Kind of the currently-selected agent, if one is selected.
    fn current_agent_kind(&self) -> Option<AgentKind> {
        self.current_agent().map(|a| a.kind)
    }

    #[allow(dead_code)]
    fn current_plugin_config(&self) -> Option<&PluginConfig> {
        self.active_plugin
            .as_deref()
            .and_then(|name| self.config.plugins.iter().find(|p| p.name == name))
    }

    fn current_plugin_panel(&self) -> Option<&PluginPanel> {
        let name = self.active_plugin.as_deref()?;
        self.plugin_panels
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, p)| p)
    }

    /// Whether the currently-selected section actually filters its data by the
    /// active period. Drives whether the period-pill row is rendered.
    fn current_section_uses_period(&self) -> bool {
        match self.mode {
            Mode::Agent => self.active_agent_section.uses_period(),
            Mode::Plugin => self
                .current_plugin_panel()
                .and_then(|p| {
                    self.active_plugin_section
                        .as_deref()
                        .and_then(|id| p.section(id))
                })
                .map(|s| s.uses_period)
                .unwrap_or(true),
        }
    }

    /// Brand accent for the currently-selected agent/plugin. Falls back to the
    /// global accent when nothing is selected or the brand color is too light
    /// to read on the dark surface (rule from .design/agents.md).
    fn current_accent(&self) -> u32 {
        match self.mode {
            Mode::Agent => self
                .current_agent()
                .map(|a| self.theme.agent_accent(a))
                .unwrap_or(self.theme.colors.accent),
            Mode::Plugin => self
                .current_plugin_config()
                .map(|p| self.theme.plugin_accent(p))
                .unwrap_or(self.theme.colors.accent),
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AuraView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let last_height = self.last_window_height.clone();
        let body_scroll = self.body_scroll.clone();
        // `display.auto_resize` governs the content-fit auto-resize that grows /
        // shrinks the window to fit its content on every layout pass. It is
        // independent of `window_chrome`, so the auto-fit works the same with
        // or without native chrome. Default on (see `DisplayConfig::auto_resize`).
        let auto_fit = self.config.display.auto_resize();
        let user_max_height = self.config.display.max_height;
        // How the modal re-anchors after the auto-fit resize (see
        // `placement::Anchor`). Only `Bottom` triggers an active move.
        let anchor = crate::placement::Anchor::from_config(&self.config.display.anchor);
        #[cfg(target_os = "windows")]
        let needs_uncloak = self.needs_uncloak.clone();
        let mut root = div()
            .flex()
            .flex_col()
            .w_full()
            // Fill the window vertically so the body's `flex_shrink` engages
            // when content exceeds the (capped) window height, letting it
            // scroll instead of being clipped off the bottom.
            .h_full()
            .bg(rgb(self.theme.colors.bg))
            .text_color(rgb(self.theme.colors.text))
            .font_family(SharedString::from(
                self.theme.typography.font_family.clone(),
            ))
            .text_sm()
            .child(self.render_header(cx))
            .child(self.render_selector_row(cx))
            .when(self.current_section_uses_period(), |d| {
                d.child(self.render_period_row(cx))
            })
            .child(self.render_tab_row(cx))
            .child(self.render_body(cx))
            // After children have been laid out, sum their vertical extent
            // and resize the window so it tightly fits the content. The body
            // is allowed to shrink below its content (overflow_y_scroll), so
            // we add its scroll max-offset back to recover the natural total.
            .on_children_prepainted(move |bounds, window, app| {
                if !auto_fit {
                    return;
                }
                let Some(bottom) = bounds.iter().map(|b| b.origin.y + b.size.height).max() else {
                    return;
                };
                let mut measured = (bottom + body_scroll.max_offset().height).ceil();

                // Cap measured at a safe distance above the primary
                // display's bottom edge so the window never grows into a
                // bottom taskbar / dock. GPUI 0.2 has no public
                // set_position API, so we can't anchor the bottom by
                // moving the origin up — we can only stop growing
                // downward.
                //
                // Strategy B: prefer the actual work area exposed by
                // the desktop environment (display rect minus reserved
                // panels/struts) so we can grow right up to — but never
                // past — the top of a bottom taskbar. The query is
                // cached process-wide after the first call, so this
                // adds no per-resize D-Bus traffic.
                //
                // Fallback: a blind 120px reserve from the display
                // bottom. Comfortably covers KDE Plasma's "Huge" panel
                // preset (~120px) and macOS Dock at default size on
                // platforms where we can't ask the DE for the real
                // number (non-Linux, non-Plasma, or D-Bus errors).
                let bottom_reserve = px(120.);
                if let Some(display) = app.primary_display() {
                    let dbounds = display.bounds();
                    let screen_bottom = dbounds.origin.y + dbounds.size.height;
                    let window_top = window.bounds().origin.y;
                    let available_bottom = crate::work_area::available_bottom(dbounds)
                        .map(px)
                        .unwrap_or(screen_bottom - bottom_reserve);
                    // Floor at 200px so a misconfigured / tiny display
                    // doesn't collapse the modal to nothing.
                    let max_h = (available_bottom - window_top).max(px(200.));
                    if measured > max_h {
                        measured = max_h;
                    }
                }

                // User-configured ceiling: applied unconditionally so it
                // still kicks in when the primary-display query above fails.
                if let Some(user_max) = user_max_height {
                    let user_max_px = px(user_max as f32);
                    if measured > user_max_px {
                        measured = user_max_px;
                    }
                }

                let last = last_height.get();
                if (measured - last).abs() < px(1.0) {
                    return;
                }
                last_height.set(measured);
                let new_size = size(px(WINDOW_WIDTH), measured);
                // Captured for the direction-aware resize/move ordering below
                // (only the non-Windows bottom-anchor path uses it).
                #[cfg(not(target_os = "windows"))]
                let prev_height = last;
                #[cfg(target_os = "windows")]
                let uncloak = needs_uncloak.clone();
                window.on_next_frame(move |window, _cx| {
                    // For a bottom-anchored modal we want the *bottom* edge
                    // pinned to the taskbar, so compute the desired top-left
                    // *absolutely* from the work area (never read back from the
                    // live origin, which is what made the window walk across the
                    // screen, issue #27). `None`/`top` anchors don't reposition:
                    // GPUI's grow-downward-from-a-fixed-top is already what they
                    // want.
                    let desired = if anchor.needs_reposition() {
                        _cx.primary_display().map(|display| {
                            crate::placement::modal_origin(
                                display.bounds(),
                                None,
                                f32::from(new_size.height),
                                anchor,
                            )
                        })
                    } else {
                        None
                    };

                    // On Windows GPUI's resize() keeps the top-left fixed; we
                    // then move + lift the open-time DWM cloak, which must fire
                    // after the resize regardless of anchor (the window is
                    // cloaked on open in every anchor mode).
                    #[cfg(target_os = "windows")]
                    {
                        window.resize(new_size);
                        crate::platform::reposition_after_resize(window, _cx, desired, &uncloak);
                    }
                    // Elsewhere, the resize and the bottom-anchor move are two
                    // separate primitives (GPUI's resize() keeps the top fixed
                    // and grows the bottom; an X11/EWMH move repositions the
                    // top-left). We order them by direction so the window never
                    // leaves the screen mid-transition — that off-screen
                    // excursion is what looked like a slow, stretchy grow:
                    //   • growing  → move the top *up* first, then grow the
                    //                bottom down to meet the taskbar (never
                    //                bulges past it).
                    //   • shrinking → shrink the bottom up first (lifts off the
                    //                taskbar), then slide down to re-pin.
                    // `None`/`top` anchors just resize (grow downward).
                    #[cfg(not(target_os = "windows"))]
                    match desired {
                        Some(origin) if new_size.height > prev_height => {
                            crate::platform::set_window_origin(window, _cx, origin);
                            window.resize(new_size);
                        }
                        Some(origin) => {
                            window.resize(new_size);
                            crate::platform::set_window_origin(window, _cx, origin);
                        }
                        None => window.resize(new_size),
                    }
                });
            });

        if self.show_more_modal {
            root = root.child(self.render_more_modal(cx));
        }
        if self.show_settings_panel {
            root = root.child(self.render_settings_panel(cx));
        }
        root
    }
}

// ── Sub-renderers ─────────────────────────────────────────────────────────────

impl AuraView {
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(self.theme.colors.border))
            .bg(rgb(self.theme.colors.surface));

        // Left: brand (+ spinner when a fetch is in flight)
        let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
        let brand = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(svg_icon("icons/aura.svg", self.theme.colors.accent, 18.0))
            .when(self.is_loading, |d| {
                d.child(
                    div()
                        .text_color(rgb(self.theme.colors.text_dim))
                        .child(frame),
                )
            });

        // Right: optional update chip, then action buttons (refresh, config, more).
        let mut actions = div().flex().flex_row().items_center().gap_3();
        if self.show_update_button() {
            actions = actions.child(self.render_update_button(cx));
        }
        actions = actions
            .child(
                icon_button("act-refresh", "icons/rotate_cw.svg", &self.theme).on_click(
                    cx.listener(|view, _: &ClickEvent, _, cx| {
                        // Early-return if a refresh is already running so we
                        // don't double-spawn.
                        if view.is_loading {
                            return;
                        }
                        view.refresh(cx);
                    }),
                ),
            )
            .child(
                icon_button("act-config", "icons/settings.svg", &self.theme).on_click(
                    cx.listener(|view, _: &ClickEvent, _, cx| view.toggle_settings_panel(cx)),
                ),
            )
            .child(
                icon_button("act-more", "icons/ellipsis.svg", &self.theme).on_click(
                    cx.listener(|view, _: &ClickEvent, _, cx| view.toggle_more_modal(cx)),
                ),
            );

        row.child(brand).child(actions).into_any_element()
    }

    /// The "Update available · vX.Y.Z | ×" chip rendered in the header. A
    /// single rounded container holds two click targets separated by a
    /// vertical divider: the label opens the README's updating section,
    /// and the trailing "×" persists a per-version dismissal so the chip
    /// vanishes until the next release.
    fn render_update_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(info) = &self.update else {
            return div().into_any_element();
        };
        let lex = lexicon::pick(self.config.display.goblin_mode);
        let accent = self.theme.colors.accent;
        let text = self.theme.colors.text;
        let text_dim = self.theme.colors.text_dim;
        let surface_hi = self.theme.colors.surface_hi;
        let border = self.theme.colors.border;
        let label = (lex.update_available_fmt)(&info.latest.to_string());

        let label_btn = div()
            .id("update-open")
            .flex()
            .flex_row()
            .items_center()
            .px_2()
            .py_0p5()
            .rounded_l_md()
            .text_xs()
            .text_color(rgb(text))
            .hover(move |d| d.bg(rgb(surface_hi)))
            .child(SharedString::from(label))
            .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.open_update_instructions(cx)));

        let divider = div().w(px(1.0)).h(px(14.0)).bg(rgb(border));

        let dismiss_btn = div()
            .id("update-dismiss")
            .flex()
            .items_center()
            .justify_center()
            .h_full()
            .w(px(20.0))
            .rounded_r_md()
            .text_xs()
            .text_color(rgb(text_dim))
            .hover(move |d| d.bg(rgb(surface_hi)).text_color(rgb(text)))
            .child("×")
            .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.dismiss_update(cx)));

        div()
            .flex()
            .flex_row()
            .items_center()
            .rounded_md()
            .bg(rgb(Theme::blend(accent, self.theme.colors.bg, 0.75)))
            .child(label_btn)
            .child(divider)
            .child(dismiss_btn)
            .into_any_element()
    }

    /// Pill row + agent/plugin mode toggle.
    fn render_selector_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(self.theme.colors.border));

        // Left: agent pills (Agent mode) or plugin pills (Plugin mode)
        let left = match self.mode {
            Mode::Agent => self.render_agent_pills(cx),
            Mode::Plugin => self.render_plugin_pills(cx),
        };
        row = row.child(left);

        // Right: mode toggle
        row = row.child(self.render_mode_toggle(cx));
        row.into_any_element()
    }

    fn render_agent_pills(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut picker = div()
            .id("agent-pills-row")
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .gap_2()
            .overflow_x_scroll();
        for agent in &self.config.agents {
            let name = agent.name.clone();
            let active = self.active_profile == agent.name;
            let pill_name = name.clone();
            let accent = self.theme.agent_accent(agent);
            let icon = agent_icon(agent, &self.theme);
            picker = picker.child(
                div()
                    .id(SharedString::from(format!("profile-{}", agent.name)))
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(active, |d| {
                        d.bg(rgb(Theme::blend(accent, self.theme.colors.bg, 0.75)))
                            .text_color(rgb(self.theme.colors.text))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(self.theme.colors.surface_hi))
                            .text_color(rgb(self.theme.colors.text_dim))
                    })
                    .child(icon)
                    .child(SharedString::from(name))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.set_profile(pill_name.clone(), cx);
                    })),
            );
        }
        picker.into_any_element()
    }

    fn render_plugin_pills(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.config.plugins.is_empty() {
            let lex = lexicon::pick(self.config.display.goblin_mode);
            return div()
                .text_xs()
                .text_color(rgb(self.theme.colors.text_dim))
                .child(lex.no_plugins_configured)
                .into_any_element();
        }
        let mut picker = div()
            .id("plugin-pills-row")
            .flex()
            .flex_row()
            .flex_1()
            .min_w_0()
            .gap_2()
            .overflow_x_scroll();
        for plugin in &self.config.plugins {
            let name = plugin.name.clone();
            let active = self.active_plugin.as_deref() == Some(name.as_str());
            let pill_name = name.clone();
            let accent = self.theme.plugin_accent(plugin);
            let icon_path = plugin_icon_path(plugin);
            let icon_color = if active {
                accent
            } else {
                self.theme.colors.text_dim
            };
            picker = picker.child(
                div()
                    .id(SharedString::from(format!("plugin-{}", plugin.name)))
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(active, |d| {
                        d.bg(rgb(Theme::blend(accent, self.theme.colors.bg, 0.75)))
                            .text_color(rgb(self.theme.colors.text))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(self.theme.colors.surface_hi))
                            .text_color(rgb(self.theme.colors.text_dim))
                    })
                    .child(svg_icon_dynamic(icon_path, icon_color, 12.0))
                    .child(SharedString::from(name))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.set_plugin(pill_name.clone(), cx);
                    })),
            );
        }
        picker.into_any_element()
    }

    fn render_mode_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let lex = lexicon::pick(self.config.display.goblin_mode);
        let modes = [
            ("Agents", Mode::Agent, "mode-agent"),
            (lex.tab_plugins, Mode::Plugin, "mode-plugin"),
        ];
        // `flex_shrink_0` keeps the toggle visible when the sibling pill row
        // overflows and scrolls.
        let mut row = div().flex().flex_row().flex_shrink_0().gap_1();
        for (label, mode, id) in modes {
            let active = self.mode == mode;
            row = row.child(
                div()
                    .id(SharedString::from(id))
                    .px_2()
                    .py_1()
                    .text_xs()
                    .rounded_md()
                    .when(active, |d| {
                        d.bg(rgb(self.theme.colors.accent))
                            .text_color(rgb(self.theme.colors.on_accent))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(self.theme.colors.surface))
                            .text_color(rgb(self.theme.colors.text_dim))
                    })
                    .child(label)
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.set_mode(mode, cx);
                    })),
            );
        }
        row.into_any_element()
    }

    fn render_period_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let lex = lexicon::pick(self.config.display.goblin_mode);
        let periods = [
            (lex.period_all, Period::AllTime, "period-all"),
            (lex.period_7d, Period::Last7Days, "period-7"),
            (lex.period_30d, Period::Last30Days, "period-30"),
        ];

        // Selected filter background uses the active agent's (or plugin's)
        // accent color so the filter row reads as "belonging to" the
        // currently-selected profile — see .design/agents.md.
        let accent = self.current_accent();
        let on_accent = self.theme.on_accent_text(accent);

        let mut row = div()
            .flex()
            .flex_row()
            .flex_shrink_0()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(self.theme.colors.border));

        for (label, period, id) in periods {
            let active = self.active_period == period;
            row = row.child(
                div()
                    .id(SharedString::from(id))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(active, |d| d.bg(rgb(accent)).text_color(rgb(on_accent)))
                    .when(!active, |d| {
                        d.bg(rgb(self.theme.colors.surface))
                            .text_color(rgb(self.theme.colors.text_dim))
                    })
                    .child(label)
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.set_period(period, cx);
                    })),
            );
        }
        row.into_any_element()
    }

    fn render_tab_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let accent = self.current_accent();
        let mut row = div()
            .id("section-tab-row")
            .flex()
            .flex_row()
            .flex_shrink_0()
            .gap_4()
            .px_4()
            .py_2()
            .overflow_x_scroll()
            .border_b_1()
            .border_color(rgb(self.theme.colors.border))
            .bg(rgb(self.theme.colors.surface));

        let lex = lexicon::pick(self.config.display.goblin_mode);
        match self.mode {
            Mode::Agent => {
                let mut sections = vec![
                    AgentSection::Quota,
                    AgentSection::Forecast,
                    AgentSection::Summary,
                    AgentSection::Models,
                ];
                // Insights is opt-in (`[insights] enabled`), so the tab only
                // appears when the user has turned it on.
                if self.config.insights.enabled {
                    sections.push(AgentSection::Insights);
                }
                // The Fleet tab is hidden unless the user opted in via
                // `[fleet].enabled`. Only meaningful for the Claude agent
                // (account-wide rate-limit windows).
                if self.config.fleet.enabled
                    && self.current_agent_kind() == Some(AgentKind::ClaudeCode)
                {
                    sections.push(AgentSection::Fleet);
                }
                // The Activity tab (live Claude Code process monitor) is hidden
                // unless the user opted in via `[activity].enabled`. Only
                // meaningful for the Claude agent.
                if self.config.activity.enabled
                    && self.current_agent_kind() == Some(AgentKind::ClaudeCode)
                {
                    sections.push(AgentSection::Activity);
                }
                for s in sections {
                    let active = self.active_agent_section == s;
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("agent-section-{}", s.id())))
                            .flex_shrink_0()
                            .text_sm()
                            .pb_1()
                            .when(active, |d| {
                                d.text_color(rgb(self.theme.colors.text))
                                    .border_b_2()
                                    .border_color(rgb(accent))
                            })
                            .when(!active, |d| d.text_color(rgb(self.theme.colors.text_dim)))
                            .child(s.label(lex))
                            .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                                view.set_agent_section(s, cx);
                            })),
                    );
                }
            }
            Mode::Plugin => {
                if let Some(panel) = self.current_plugin_panel() {
                    for section in &panel.sections {
                        let sid = section.id.clone();
                        let label = section.label.clone();
                        let active = self.active_plugin_section.as_deref() == Some(sid.as_str());
                        let click_sid = sid.clone();
                        row = row.child(
                            div()
                                .id(SharedString::from(format!("plugin-section-{}", sid)))
                                .flex_shrink_0()
                                .text_sm()
                                .pb_1()
                                .when(active, |d| {
                                    d.text_color(rgb(self.theme.colors.text))
                                        .border_b_2()
                                        .border_color(rgb(accent))
                                })
                                .when(!active, |d| d.text_color(rgb(self.theme.colors.text_dim)))
                                .child(SharedString::from(label))
                                .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                                    view.set_plugin_section(click_sid.clone(), cx);
                                })),
                        );
                    }
                }
            }
        }
        row.into_any_element()
    }

    fn render_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let lex = lexicon::pick(self.config.display.goblin_mode);
        let inner: AnyElement = if let Some(err) = &self.error {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .p_6()
                .child(
                    div()
                        .text_color(rgb(self.theme.colors.error))
                        .child(err.clone()),
                )
                .into_any_element()
        } else if self.is_loading {
            // While a refresh is in flight, hide whatever the previous
            // agent/plugin/period produced and show a clean loading state.
            // Otherwise switching from Claude → Codex (etc.) flashes the
            // outgoing profile's numbers under the new tab.
            render_loading(&self.theme, lex, self.spinner_frame)
        } else {
            let accent = self.current_accent();
            match self.mode {
                Mode::Agent => match self.active_agent_section {
                    AgentSection::Quota => render_quota(
                        &self.theme,
                        lex,
                        self.quota.as_ref(),
                        accent,
                        self.spinner_frame,
                    ),
                    AgentSection::Forecast => render_forecast(
                        &self.theme,
                        lex,
                        self.forecast.as_ref(),
                        self.quota.as_ref(),
                        self.pacing_caps,
                        self.config.pacing.enabled,
                        accent,
                        self.spinner_frame,
                    ),
                    AgentSection::Summary => match self.snapshot.as_ref() {
                        Some(snap) => render_summary(&self.theme, snap),
                        None => render_loading(&self.theme, lex, self.spinner_frame),
                    },
                    AgentSection::Models => match self.snapshot.as_ref() {
                        Some(snap) => render_models(&self.theme, snap, accent),
                        None => render_loading(&self.theme, lex, self.spinner_frame),
                    },
                    AgentSection::Insights => match self.snapshot.as_ref() {
                        Some(snap) => render_insights(
                            &self.theme,
                            snap,
                            accent,
                            self.config.insights.top_n,
                        ),
                        None => render_loading(&self.theme, lex, self.spinner_frame),
                    },
                    AgentSection::Fleet => {
                        // Defensive: the tab is hidden for non-Claude agents /
                        // when disabled, but the selection can persist across a
                        // profile switch. Fall back to the quota view rather
                        // than showing an empty Fleet panel.
                        if self.config.fleet.enabled
                            && self.current_agent_kind() == Some(AgentKind::ClaudeCode)
                        {
                            self.render_fleet(accent, cx)
                        } else {
                            render_quota(
                                &self.theme,
                                lex,
                                self.quota.as_ref(),
                                accent,
                                self.spinner_frame,
                            )
                        }
                    }
                    AgentSection::Activity => render_activity(
                        &self.theme,
                        &self.activity_sessions,
                        self.activity_primed,
                        self.config.activity.refresh_secs.max(1),
                        accent,
                    ),
                },
                Mode::Plugin => self.render_plugin_body(),
            }
        };

        // `min_h_0` lets the body shrink below its content when the window is
        // capped at the screen bottom; `overflow_y_scroll` lets the user reach
        // the clipped portion; `track_scroll` exposes the overflow back to the
        // auto-resize callback so the window still re-grows when content fits.
        div()
            .id("body")
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.body_scroll)
            .child(inner)
            .into_any_element()
    }

    fn render_plugin_body(&self) -> AnyElement {
        let lex = lexicon::pick(self.config.display.goblin_mode);
        let Some(panel) = self.current_plugin_panel() else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .p_6()
                .child(
                    div()
                        .text_color(rgb(self.theme.colors.text_dim))
                        .child(lex.no_plugin_selected),
                )
                .into_any_element();
        };

        if let Some(err) = &panel.error {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .p_6()
                .child(
                    div()
                        .text_color(rgb(self.theme.colors.error))
                        .child(SharedString::from(err.clone())),
                )
                .into_any_element();
        }

        let section_id = self.active_plugin_section.as_deref().unwrap_or("");
        let section = panel.section(section_id).or_else(|| panel.sections.first());
        match section {
            Some(s) => render_plugin_section(&self.theme, s, self.current_accent()),
            None => render_loading(&self.theme, lex, self.spinner_frame),
        }
    }
}

// ── Quota (subscription windows — mirrors `claude /usage`) ───────────────────

/// Centralized loading placeholder. Sized to match the typical rendered
/// height of two quota windows (label + progress bar + resets row) so the
/// modal barely resizes when the initial fetch completes — fewer visible
/// jumps on first paint and on agent/plugin switches.
fn render_loading(theme: &Theme, lex: &Lexicon, spinner_frame: usize) -> AnyElement {
    let frame = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
    div()
        .flex()
        .flex_col()
        .h(px(260.0))
        .w_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .w(px(48.0))
                .h(px(48.0))
                .rounded_md()
                .bg(rgb(theme.colors.surface))
                .border_1()
                .border_color(rgb(theme.colors.border))
                .text_lg()
                .text_color(rgb(theme.colors.text_dim))
                .child(frame),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(lex.loading),
        )
        .into_any_element()
}

fn render_quota(
    theme: &Theme,
    lex: &Lexicon,
    quota: Option<&QuotaSnapshot>,
    accent: u32,
    spinner_frame: usize,
) -> AnyElement {
    let Some(quota) = quota else {
        return render_loading(theme, lex, spinner_frame);
    };

    let mut col = div().flex().flex_col().px_4().py_3().gap_3();

    // Show the subscription tier (e.g. "pro", "max") when the API was reached.
    if let Some(sub) = &quota.subscription_type {
        col = col.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from((lex.subscription_fmt)(sub))),
        );
    }

    if quota.source != QuotaSource::Api {
        col = col.child(render_fallback_warning(theme, quota));
    }

    if quota.windows.is_empty() {
        if quota.source == QuotaSource::Api {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.colors.border))
                    .bg(rgb(theme.colors.surface))
                    .text_color(rgb(theme.colors.text_dim))
                    .text_xs()
                    .child(lex.no_quota_data),
            );
        }
    } else {
        for w in &quota.windows {
            col = col.child(render_quota_window(theme, lex, w, accent));
        }
    }

    col.into_any_element()
}

/// Discrete warning chip shown when the quota snapshot didn't come from the
/// `/api/oauth/usage` endpoint. Distinguishes the fallback kind so the user
/// knows whether numbers are local estimates or absent entirely.
fn render_fallback_warning(theme: &Theme, quota: &QuotaSnapshot) -> AnyElement {
    let (kind, default_note) = match quota.source {
        QuotaSource::Fallback => (
            "Local estimate",
            "Subscription limits unknown — showing local token counts.",
        ),
        QuotaSource::Unavailable => (
            "Quota unavailable",
            "Quota data is not available right now.",
        ),
        QuotaSource::Api => return div().into_any_element(),
    };
    let note = quota
        .note
        .clone()
        .unwrap_or_else(|| default_note.to_string());

    div()
        .flex()
        .flex_row()
        .w_full()
        .items_start()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .bg(rgb(theme.colors.surface))
        .child(
            div()
                .mt(px(1.0))
                .child(svg_icon("icons/info.svg", theme.colors.warning, 12.0)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.colors.warning))
                        .child(SharedString::from(kind)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .child(SharedString::from(note)),
                ),
        )
        .into_any_element()
}

fn render_quota_window(
    theme: &Theme,
    lex: &Lexicon,
    w: &QuotaWindow,
    accent: u32,
) -> impl IntoElement {
    let pct_label = match w.used_percentage {
        Some(p) => format!("{:.0}% used", p),
        None => match w.used_tokens {
            Some(t) => format!("{} tokens", thousands(t)),
            None => "—".to_string(),
        },
    };

    let resets_label = w.resets_at.map(format_reset);

    let mut row = div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(
            div()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(w.label.clone())),
        );

    // The progress bar is meaningful only when we have a real percentage. For
    // fallback windows (local token counts), show the count without an empty
    // bar that reads as broken UI.
    row = if let Some(pct) = w.used_percentage {
        let bar_fraction = (pct.clamp(0.0, 100.0) / 100.0) as f32;
        row.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .h(px(8.0))
                        .flex_1()
                        .bg(rgb(theme.colors.surface_hi))
                        .rounded_md()
                        .child(
                            div()
                                .h(px(8.0))
                                .w(gpui::relative(bar_fraction))
                                .bg(rgb(accent))
                                .rounded_md(),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.colors.text))
                        .child(SharedString::from(pct_label)),
                ),
        )
    } else {
        row.child(
            div()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(pct_label)),
        )
    };

    if let Some(label) = resets_label {
        row = row.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from((lex.resets_fmt)(&label))),
        );
    }

    row
}

/// Format a UTC instant as a friendly local-time string, respecting the OS
/// locale for month abbreviations and the am/pm marker. In 12h locales the
/// near-term output looks like `6:50pm (America/Sao_Paulo)`; 24h locales get
/// `18:50 (America/Sao_Paulo)`. Beyond 24h the date is added: `May 22, 4pm`
/// or `22 mai, 16:00`.
fn format_reset(ts: DateTime<Utc>) -> String {
    let local = ts.with_timezone(&Local);
    let now = Local::now();
    let within_24h = (local - now).num_hours().abs() < 24;
    let tz_label = chrono::Local::now().offset().to_string();
    let locale = system_locale();
    let uses_12h = locale_uses_12h();
    let minute = local.minute();

    let time = if uses_12h {
        let hour = local.format_localized("%-I", locale).to_string();
        let suffix = local
            .format_localized("%P", locale)
            .to_string()
            .to_lowercase();
        if minute == 0 {
            format!("{hour}{suffix}")
        } else {
            format!("{hour}:{minute:02}{suffix}")
        }
    } else {
        format!("{:02}:{:02}", local.hour(), minute)
    };

    if within_24h {
        format!("{time} ({tz_label})")
    } else {
        let date = local.format_localized("%b %-d", locale).to_string();
        format!("{date}, {time}")
    }
}

// ── Forecast (projected end-of-window usage at current burn rate) ───────────

#[allow(clippy::too_many_arguments)]
fn render_forecast(
    theme: &Theme,
    lex: &Lexicon,
    forecast: Option<&ForecastSnapshot>,
    quota: Option<&QuotaSnapshot>,
    pacing_caps: Option<pacing::Caps>,
    pacing_enabled: bool,
    accent: u32,
    spinner_frame: usize,
) -> AnyElement {
    let Some(forecast) = forecast else {
        return render_loading(theme, lex, spinner_frame);
    };

    let mut col = div().flex().flex_col().px_4().py_3().gap_3();

    if forecast.windows.is_empty() {
        col = col.child(
            div()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(theme.colors.border))
                .bg(rgb(theme.colors.surface))
                .text_color(rgb(theme.colors.text_dim))
                .text_xs()
                .child(
                    "No forecast available — quota source did not report any projectable windows.",
                ),
        );
    } else {
        for w in &forecast.windows {
            col = col.child(render_forecast_window(theme, lex, w, accent));
        }
    }

    // Session-budget gauge (F2) — appended below the projected windows when the
    // feature is enabled. Computed from the live snapshot + learned pattern.
    if pacing_enabled {
        if let Some(q) = quota {
            let budget = pacing::session_budget(q, pacing_caps, Utc::now());
            col = col.child(render_session_budget(theme, lex, &budget, accent));
        }
    }

    col.into_any_element()
}

/// The per-session budget gauge (F2). Reuses the forecast card's bar/badge
/// primitives: a header with a status badge, a usage-vs-ceiling bar, and a
/// one-line rationale. `Insufficient` collapses to the warming-up note with no
/// number.
fn render_session_budget(
    theme: &Theme,
    lex: &Lexicon,
    budget: &SessionBudget,
    accent: u32,
) -> impl IntoElement {
    let (badge_text, badge_color) = match budget.status {
        PacingStatus::Ok => (lex.pacing_ok, accent),
        PacingStatus::Watch => (lex.pacing_watch, theme.colors.warning),
        PacingStatus::Over => (lex.pacing_over, theme.colors.error),
        PacingStatus::Insufficient => (lex.pacing_insufficient, theme.colors.text_dim),
    };

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(lex.pacing_title)),
        )
        .child(
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(rgb(badge_color))
                .text_xs()
                .text_color(rgb(badge_color))
                .child(SharedString::from(badge_text)),
        );

    let mut card = div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(header);

    // No fabricated number when we lack history / live data.
    let (Some(recommended), Some(used)) = (budget.recommended_pct, budget.session_used_pct) else {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(
                    budget
                        .note
                        .clone()
                        .unwrap_or_else(|| lex.forecast_warming_up.to_string()),
                )),
        );
        return card;
    };

    card = card.child(
        div()
            .text_color(rgb(theme.colors.text))
            .child(SharedString::from((lex.pacing_spend_up_to_fmt)(
                recommended,
            ))),
    );

    // Gauge: the recommended ceiling is the full track; the solid fill is the
    // current 5h usage relative to that ceiling. Over budget fills the track
    // and shows an overflow marker.
    let ceiling = recommended.max(f64::EPSILON);
    let used_frac = (used / ceiling).clamp(0.0, 1.0) as f32;
    let bar_color = match budget.status {
        PacingStatus::Over => theme.colors.error,
        PacingStatus::Watch => theme.colors.warning,
        _ => accent,
    };

    let bar = div()
        .h(px(8.0))
        .flex_1()
        .bg(rgb(theme.colors.surface_hi))
        .rounded_md()
        .flex()
        .flex_row()
        .child(
            div()
                .h(px(8.0))
                .w(gpui::relative(used_frac))
                .bg(rgb(bar_color))
                .rounded_md(),
        );

    let mut bar_row = div().flex().flex_row().items_center().gap_2().child(bar);
    if budget.status == PacingStatus::Over {
        bar_row = bar_row.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.error))
                .child("↗"),
        );
    }
    bar_row = bar_row.child(
        div()
            .text_xs()
            .text_color(rgb(theme.colors.text))
            .child(SharedString::from(format!(
                "{:.0}% / {:.0}%",
                used, recommended
            ))),
    );
    card = card.child(bar_row);

    if let Some(note) = &budget.note {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(note.clone())),
        );
    }

    card
}

fn render_forecast_window(
    theme: &Theme,
    lex: &Lexicon,
    w: &ForecastWindow,
    accent: u32,
) -> impl IntoElement {
    let (badge_text, badge_color) = match w.status {
        ForecastStatus::Ok => (lex.forecast_ok, accent),
        ForecastStatus::Watch => (lex.forecast_watch, theme.colors.warning),
        ForecastStatus::Overshoot => (lex.forecast_overshoot, theme.colors.error),
        ForecastStatus::Insufficient => (lex.forecast_insufficient, theme.colors.text_dim),
    };

    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(w.label.clone())),
        )
        .child(
            div()
                .px_2()
                .py_0p5()
                .rounded_md()
                .border_1()
                .border_color(rgb(badge_color))
                .text_xs()
                .text_color(rgb(badge_color))
                .child(SharedString::from(badge_text)),
        );

    let mut card = div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(header);

    if w.status == ForecastStatus::Insufficient {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(lex.forecast_warming_up),
        );
        return card;
    }

    // Two-segment bar: solid `used_now` + translucent `projected delta`.
    // Cap both at 100%; overshoot gets a separate marker on the right.
    let used_now = w.used_percentage_now.unwrap_or(0.0).clamp(0.0, 100.0) as f32;
    let projected = w
        .projected_percentage
        .unwrap_or(used_now as f64)
        .clamp(0.0, 100.0) as f32;
    let used_frac = used_now / 100.0;
    let projected_extra_frac = ((projected - used_now).max(0.0)) / 100.0;
    let overflowed = w.projected_percentage.map(|p| p > 100.0).unwrap_or(false);

    // Show "current / projected" so the user sees both the live value and
    // where the bar is heading.
    let projected_label = match (w.used_percentage_now, w.projected_percentage) {
        (Some(now_pct), Some(proj_pct)) => format!("{:.0}% / {:.0}%", now_pct, proj_pct),
        (Some(now_pct), None) => format!("{:.0}% / —", now_pct),
        (None, Some(proj_pct)) => format!("— / {:.0}%", proj_pct),
        (None, None) => "—".to_string(),
    };

    // Build the bar: a track, then a flex row of two filled segments.
    let bar = div()
        .h(px(8.0))
        .flex_1()
        .bg(rgb(theme.colors.surface_hi))
        .rounded_md()
        .flex()
        .flex_row()
        .child(
            div()
                .h(px(8.0))
                .w(gpui::relative(used_frac))
                .bg(rgb(accent))
                .rounded_md(),
        )
        .child(
            div()
                .h(px(8.0))
                .w(gpui::relative(projected_extra_frac))
                .bg(rgba((accent << 8) | 0x66))
                .rounded_md(),
        );

    let mut bar_row = div().flex().flex_row().items_center().gap_2().child(bar);

    if overflowed {
        bar_row = bar_row.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.error))
                .child("↗"),
        );
    }

    bar_row = bar_row.child(
        div()
            .text_xs()
            .text_color(rgb(theme.colors.text))
            .child(SharedString::from(projected_label)),
    );
    card = card.child(bar_row);

    let subtext = match w.status {
        ForecastStatus::Overshoot => w
            .overshoot_at
            .map(|t| (lex.forecast_will_hit_100_fmt)(&format_reset(t)))
            .unwrap_or_else(|| "Projected to overshoot".to_string()),
        _ => match w.resets_at {
            Some(t) => format!("{} {}", lex.forecast_projected_at_reset, format_reset(t)),
            None => lex.forecast_projected_at_reset.to_string(),
        },
    };
    card = card.child(
        div()
            .text_xs()
            .text_color(rgb(theme.colors.text_dim))
            .child(SharedString::from(subtext)),
    );

    card
}

// ── Summary (the old "Overview" — stat-card grid) ────────────────────────────

fn render_summary(theme: &Theme, snap: &UsageSnapshot) -> AnyElement {
    let rows = [
        (
            "Favorite model",
            snap.favorite_model
                .clone()
                .unwrap_or_else(|| "—".to_string()),
        ),
        ("Total tokens", thousands(snap.total_tokens)),
        ("Sessions", thousands(snap.total_sessions)),
        (
            "Longest session",
            snap.longest_session_secs
                .map(duration)
                .unwrap_or_else(|| "—".to_string()),
        ),
        (
            "Active days",
            format!(
                "{} / {}",
                snap.active_days,
                if snap.total_days > 0 {
                    snap.total_days
                } else {
                    snap.active_days
                }
            ),
        ),
        (
            "Peak hour",
            snap.peak_hour
                .map(hour_of_day)
                .unwrap_or_else(|| "—".to_string()),
        ),
        ("Current streak", format!("{} day(s)", snap.streaks.current)),
        ("Longest streak", format!("{} day(s)", snap.streaks.longest)),
    ];

    let mut col = div().flex().flex_col().px_4().py_3().gap_2();

    // Render in 2-col rows
    for chunk in rows.chunks(2) {
        let mut row = div().flex().flex_row().gap_4();
        for (label, value) in chunk {
            row = row.child(stat_card(theme, label, value));
        }
        col = col.child(row);
    }
    col.into_any_element()
}

fn stat_card(theme: &Theme, label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap_1()
        .px_3()
        .py_2()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(value.to_string())),
        )
}

// ── Models ────────────────────────────────────────────────────────────────────

fn render_models(theme: &Theme, snap: &UsageSnapshot, accent: u32) -> AnyElement {
    let mut col = div().flex().flex_col().px_4().py_3().gap_4();

    // Tokens per Day chart
    col = col.child(render_daily_chart(theme, snap, accent));

    // Per-model breakdown
    let total: u64 = snap
        .per_model
        .iter()
        .map(aura_core::reader::ModelUsage::total_tokens)
        .sum();

    let mut models_col = div().flex().flex_col().gap_2();
    for m in &snap.per_model {
        let tokens = m.total_tokens();
        let pct = if total > 0 {
            (tokens as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        models_col = models_col.child(render_model_row(theme, &m.model, tokens, pct, accent));
    }
    col = col.child(models_col);
    col.into_any_element()
}

fn render_model_row(
    theme: &Theme,
    model: &str,
    tokens: u64,
    pct: f64,
    accent: u32,
) -> impl IntoElement {
    let bar_width_pct = pct.clamp(2.0, 100.0);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .child(
                    div()
                        .text_color(rgb(theme.colors.text))
                        .child(SharedString::from(model.to_string())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .child(SharedString::from(format!(
                            "{:.1}% · {}",
                            pct,
                            thousands(tokens)
                        ))),
                ),
        )
        .child(
            div()
                .h(px(4.0))
                .w_full()
                .bg(rgb(theme.colors.surface_hi))
                .rounded_md()
                .child(
                    div()
                        .h(px(4.0))
                        .w(gpui::relative(bar_width_pct as f32 / 100.0))
                        .bg(rgb(accent))
                        .rounded_md(),
                ),
        )
}

/// Hover card for a single bar in the "Tokens per day" chart. Built fresh on
/// each hover so its entrance animation replays. Colors are captured as resolved
/// `u32`s so the view doesn't need to borrow the theme.
struct DailyBarTooltip {
    date: SharedString,
    total: SharedString,
    /// Per-model breakdown for the day: (short model name, compact tokens).
    models: Vec<(SharedString, SharedString)>,
    bg: u32,
    border: u32,
    text: u32,
    text_dim: u32,
    accent: u32,
}

impl Render for DailyBarTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .px_2()
            .py_1()
            .bg(rgb(self.bg))
            .border_1()
            .border_color(rgb(self.border))
            .rounded_md()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(self.text_dim))
                    .child(self.date.clone()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(self.text))
                    .child(self.total.clone()),
            );
        for (model, tokens) in &self.models {
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap_3()
                    .text_xs()
                    .child(div().text_color(rgb(self.accent)).child(model.clone()))
                    .child(div().text_color(rgb(self.text_dim)).child(tokens.clone())),
            );
        }
        // Fade + rise into place over ~160ms (ease-out cubic).
        col.with_animation(
            "daily-bar-tooltip-in",
            Animation::new(Duration::from_millis(160)).with_easing(|t| 1.0 - (1.0 - t).powi(3)),
            |this, delta| this.opacity(delta).mt(px(6.0 * (1.0 - delta))),
        )
    }
}

fn render_daily_chart(theme: &Theme, snap: &UsageSnapshot, accent: u32) -> impl IntoElement {
    struct DayBar {
        date: String,
        total: u64,
        models: Vec<(String, u64)>,
    }

    let days: Vec<DayBar> = snap
        .daily_tokens
        .iter()
        .map(|d| {
            let total = d.by_model.values().sum::<u64>();
            let mut models: Vec<(String, u64)> =
                d.by_model.iter().map(|(m, n)| (m.clone(), *n)).collect();
            models.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
            DayBar {
                date: d.date.clone(),
                total,
                models,
            }
        })
        .collect();

    let max = days.iter().map(|d| d.total).max().unwrap_or(0);

    // Captured into each bar's hover/tooltip closures.
    let surface_hi = theme.colors.surface_hi;
    let border = theme.colors.border;
    let text = theme.colors.text;
    let text_dim = theme.colors.text_dim;
    // Brighten the accent ~28% toward white for the hover state.
    let accent_hi = {
        let lift = |shift: u32| {
            let c = ((accent >> shift) & 0xff) as f32;
            ((c + (255.0 - c) * 0.28).round() as u32).min(255)
        };
        (lift(16) << 16) | (lift(8) << 8) | lift(0)
    };

    let mut bars = div().flex().flex_row().items_end().gap_1().h(px(56.0));
    for (i, d) in days.iter().enumerate() {
        let height = if max > 0 {
            (d.total as f32 / max as f32 * 48.0).max(2.0)
        } else {
            2.0
        };
        let date = d.date.clone();
        let total = d.total;
        let models = d.models.clone();
        bars = bars.child(
            div()
                .id(SharedString::from(format!("daily-bar-{i}")))
                .flex_1()
                .h(px(height))
                .bg(rgb(accent))
                .rounded_sm()
                .hover(move |s| s.bg(rgb(accent_hi)))
                .tooltip(move |_window, cx| {
                    let models_fmt: Vec<(SharedString, SharedString)> = models
                        .iter()
                        .take(3)
                        .map(|(m, n)| {
                            let short = m.strip_prefix("claude-").unwrap_or(m);
                            (SharedString::from(short.to_string()), compact_tokens(*n).into())
                        })
                        .collect();
                    cx.new(|_| DailyBarTooltip {
                        date: date.clone().into(),
                        total: format!("{} tokens", compact_tokens(total)).into(),
                        models: models_fmt,
                        bg: surface_hi,
                        border,
                        text,
                        text_dim,
                        accent,
                    })
                    .into()
                }),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child("Tokens per day"),
        )
        .child(bars)
}

// ── Insights ──────────────────────────────────────────────────────────────────

/// Render the Insights tab: top projects, top sessions (with mode badges),
/// ultracode ROI, and cache efficiency. `top_n` slices the already-ranked lists
/// from the snapshot.
fn render_insights(
    theme: &Theme,
    snap: &UsageSnapshot,
    accent: u32,
    top_n: usize,
) -> AnyElement {
    let ins = &snap.insights;
    let mut col = div().flex().flex_col().px_4().py_3().gap_4();

    col = col.child(render_insights_projects(theme, ins, accent, top_n));
    col = col.child(render_insights_sessions(theme, ins, accent, top_n));
    col = col.child(render_ultracode_roi(theme, ins, accent));
    col = col.child(render_cache_efficiency(theme, snap, accent));

    // Heuristic footnote — ultracode is inferred from session content.
    col = col.child(
        div()
            .text_xs()
            .text_color(rgb(theme.colors.text_dim))
            .child("ⓘ ultracode is inferred from session content"),
    );

    col.into_any_element()
}

/// Section heading shared by the Insights cards.
fn insights_heading(theme: &Theme, text: &str) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(rgb(theme.colors.text_dim))
        .child(SharedString::from(text.to_string()))
}

/// Wrapper card matching the Models tab's surface/border styling.
fn insights_card(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
}

fn empty_hint(theme: &Theme, text: &str) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(rgb(theme.colors.text_dim))
        .child(SharedString::from(text.to_string()))
}

fn render_insights_projects(
    theme: &Theme,
    ins: &InsightsSnapshot,
    accent: u32,
    top_n: usize,
) -> impl IntoElement {
    let mut card = insights_card(theme).child(insights_heading(theme, "TOP PROJECTS"));

    let projects: &[ProjectStat] = slice_top(&ins.top_projects, top_n);
    if projects.is_empty() {
        return card.child(empty_hint(theme, "No project activity in this period."));
    }

    let max = projects.iter().map(|p| p.tokens).max().unwrap_or(0);
    for p in projects {
        card = card.child(render_project_row(theme, p, max, accent));
    }
    card
}

fn render_project_row(
    theme: &Theme,
    project: &ProjectStat,
    max: u64,
    accent: u32,
) -> impl IntoElement {
    let bar_pct = if max > 0 {
        (project.tokens as f64 / max as f64 * 100.0).clamp(2.0, 100.0)
    } else {
        2.0
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .text_color(rgb(theme.colors.text))
                        .child(SharedString::from(project.name.clone())),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .child(SharedString::from(compact_tokens(project.tokens))),
                ),
        )
        .child(
            div()
                .h(px(4.0))
                .w_full()
                .bg(rgb(theme.colors.surface_hi))
                .rounded_md()
                .child(
                    div()
                        .h(px(4.0))
                        .w(gpui::relative(bar_pct as f32 / 100.0))
                        .bg(rgb(accent))
                        .rounded_md(),
                ),
        )
}

fn render_insights_sessions(
    theme: &Theme,
    ins: &InsightsSnapshot,
    accent: u32,
    top_n: usize,
) -> impl IntoElement {
    let mut card = insights_card(theme).child(insights_heading(theme, "TOP SESSIONS"));

    let sessions: &[SessionInsight] = slice_top(&ins.top_sessions, top_n);
    if sessions.is_empty() {
        return card.child(empty_hint(theme, "No sessions in this period."));
    }

    for s in sessions {
        card = card.child(render_session_row(theme, s, accent));
    }
    card
}

fn render_session_row(theme: &Theme, s: &SessionInsight, accent: u32) -> impl IntoElement {
    // Left: "18.2M · project". Right: mode badges. (Duration is intentionally
    // omitted — it was wall-clock incl. idle gaps and misled on reopened sessions.)
    let summary = format!("{} · {}", compact_tokens(s.tokens), s.project);

    let mut badges = div().flex().flex_row().flex_shrink_0().gap_1();
    badges = badges.child(mode_badge(theme, s.tier.label(), accent, false));
    if s.is_ultracode {
        badges = badges.child(mode_badge(theme, "ultracode", accent, true));
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex_1()
                .overflow_hidden()
                .text_sm()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(summary)),
        )
        .child(badges)
}

/// A small mode badge. The tier badge uses the dim surface; the ultracode chip
/// uses the accent fill so it stands out. Colors come from theme tokens only.
fn mode_badge(theme: &Theme, label: &str, accent: u32, filled: bool) -> impl IntoElement {
    let (bg, fg) = if filled {
        (accent, theme.on_accent_text(accent))
    } else {
        (theme.colors.surface_hi, theme.colors.text_dim)
    };
    div()
        .flex_shrink_0()
        .px_2()
        .text_xs()
        .rounded_md()
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .child(SharedString::from(label.to_string()))
}

/// Whether heavy/ultracode sessions are actually heavier, by average tokens.
/// `ultracode` detection is heuristic — see the tab footnote.
fn render_ultracode_roi(theme: &Theme, ins: &InsightsSnapshot, accent: u32) -> impl IntoElement {
    let roi = &ins.ultracode_roi;
    let mut card = insights_card(theme).child(insights_heading(theme, "ULTRACODE ROI"));

    if roi.ultracode_sessions == 0 && roi.normal_sessions == 0 {
        return card.child(empty_hint(theme, "No sessions in this period."));
    }

    // Two group rows: ultracode (accent) and normal (dim).
    let group_row = |label: &str, sessions: u32, avg: u64, filled: bool| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(mode_badge(theme, label, accent, filled))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(rgb(theme.colors.text))
                    .child(SharedString::from(format!(
                        "{sessions} sessions · avg {}",
                        compact_tokens(avg)
                    ))),
            )
    };
    card = card.child(group_row(
        "ultracode",
        roi.ultracode_sessions,
        roi.ultracode_avg_tokens,
        true,
    ));
    card = card.child(group_row(
        "normal",
        roi.normal_sessions,
        roi.normal_avg_tokens,
        false,
    ));

    // Multiplier line, omitted when there are no normal sessions to divide by.
    if let Some(mult) = roi.multiplier() {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(format!(
                    "ultracode sessions are {mult:.1}× heavier on average"
                ))),
        );
    }

    card
}

/// Share of model context served from cache — a proxy for prompt-reuse
/// efficiency. Built from the period-scoped snapshot cache totals.
fn render_cache_efficiency(theme: &Theme, snap: &UsageSnapshot, accent: u32) -> impl IntoElement {
    let eff = CacheEfficiency::new(snap.total_cache_read_tokens, snap.total_cache_write_tokens);
    let mut card = insights_card(theme).child(insights_heading(theme, "CACHE EFFICIENCY"));

    let Some(ratio) = eff.hit_ratio_pct() else {
        return card.child(empty_hint(theme, "No cache activity yet."));
    };

    // Headline: "95% of context served from cache".
    card = card.child(
        div()
            .text_sm()
            .text_color(rgb(theme.colors.text))
            .child(SharedString::from(format!(
                "{ratio:.0}% of context served from cache"
            ))),
    );

    // Read/write hit bar (accent = reuse, dim = fresh writes).
    let frac = (ratio / 100.0) as f32;
    card = card.child(
        div()
            .flex()
            .flex_row()
            .h(px(6.0))
            .w_full()
            .rounded_md()
            .bg(rgb(theme.colors.surface_hi))
            .child(
                div()
                    .h_full()
                    .w(gpui::relative(frac))
                    .bg(rgb(accent))
                    .rounded_md(),
            ),
    );

    // Raw read/write counts.
    card = card.child(
        div()
            .text_xs()
            .text_color(rgb(theme.colors.text_dim))
            .child(SharedString::from(format!(
                "cache read: {} · cache write: {}",
                compact_tokens(eff.read_tokens),
                compact_tokens(eff.write_tokens)
            ))),
    );

    card
}

/// Take the first `top_n` of an already-ranked slice (treats `0` as "all").
fn slice_top<T>(items: &[T], top_n: usize) -> &[T] {
    let n = if top_n == 0 { items.len() } else { top_n };
    &items[..n.min(items.len())]
}

// ── Plugin section rendering ─────────────────────────────────────────────────

fn render_plugin_section(theme: &Theme, section: &PluginSection, accent: u32) -> AnyElement {
    match &section.content {
        PluginContent::Lines { lines } => {
            let mut col = div().flex().flex_col().px_4().py_3().gap_2();
            for line in lines {
                let mut card = div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(rgb(theme.colors.surface))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.colors.border));

                card = card.child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(theme.colors.text_dim))
                                .child(SharedString::from(line.label.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .when(line.highlight, |d| d.text_color(rgb(accent)))
                                .when(!line.highlight, |d| d.text_color(rgb(theme.colors.text)))
                                .child(SharedString::from(line.value.clone())),
                        ),
                );

                if let Some(p) = line.progress {
                    card = card.child(progress_bar(theme, p, accent, 6.0));
                }

                col = col.child(card);
            }
            col.into_any_element()
        }
        PluginContent::Table { headers, rows } => {
            let has_progress = rows.iter().any(|r| r.progress.is_some());

            // Size each text column to its widest cell (header + rows). This
            // keeps narrow numeric columns from claiming equal space with the
            // command column when the window is small. The Impact bar (when
            // present) still flexes to absorb any leftover room.
            let n_cols = headers.len();
            let mut col_chars: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
            for row in rows {
                for (i, cell) in row.cells.iter().enumerate().take(n_cols) {
                    col_chars[i] = col_chars[i].max(cell.chars().count());
                }
            }
            // Monospace at text-xs (~11px) lands close to 7px per character.
            // A small slack keeps single-pixel rounding from clipping the
            // tail character.
            const CHAR_PX: f32 = 7.0;
            const CELL_SLACK_PX: f32 = 4.0;
            let col_widths: Vec<f32> = col_chars
                .iter()
                .map(|c| (*c as f32) * CHAR_PX + CELL_SLACK_PX)
                .collect();
            // Right-align columns that look numeric (header `#`, `%` in
            // header, or every data cell starts with a digit). Matches the
            // `rtk gain` CLI: rank/count/saved/pct/time right-align, command
            // stays left.
            let col_right: Vec<bool> = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let h_marks = h == "#" || h.contains('%');
                    let cells_numeric = !rows.is_empty()
                        && rows.iter().all(|r| {
                            r.cells
                                .get(i)
                                .map(|c| {
                                    c.trim_start()
                                        .chars()
                                        .next()
                                        .is_some_and(|ch| ch.is_ascii_digit())
                                })
                                .unwrap_or(false)
                        });
                    h_marks || cells_numeric
                })
                .collect();

            let mut col = div().flex().flex_col().px_4().py_3().gap_1();

            // Header row
            let mut header_row = div()
                .flex()
                .flex_row()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(theme.colors.border));
            for (i, h) in headers.iter().enumerate() {
                let right = col_right.get(i).copied().unwrap_or(false);
                header_row = header_row.child(
                    div()
                        .w(px(col_widths[i]))
                        .flex_none()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .when(right, |d| d.text_right())
                        .child(SharedString::from(h.clone())),
                );
            }
            if has_progress {
                header_row = header_row.child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .child("Impact"),
                );
            }
            col = col.child(header_row);

            // Data rows
            for row in rows {
                let mut r = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5();
                for (i, cell) in row.cells.iter().enumerate() {
                    let w = col_widths.get(i).copied().unwrap_or(80.0);
                    let right = col_right.get(i).copied().unwrap_or(false);
                    r = r.child(
                        div()
                            .w(px(w))
                            .flex_none()
                            .text_xs()
                            .when(row.highlight, |d| d.text_color(rgb(accent)))
                            .when(!row.highlight, |d| d.text_color(rgb(theme.colors.text)))
                            .when(right, |d| d.text_right())
                            .child(SharedString::from(cell.clone())),
                    );
                }
                if has_progress {
                    r = r.child(div().flex_1().child(progress_bar(
                        theme,
                        row.progress.unwrap_or(0.0),
                        accent,
                        6.0,
                    )));
                }
                col = col.child(r);
            }
            col.into_any_element()
        }
        PluginContent::Text { text } => div()
            .px_4()
            .py_3()
            .text_xs()
            .text_color(rgb(theme.colors.text))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
    }
}

/// A thin filled bar. `fraction` is clamped to 0.0–1.0. Used by plugin
/// `progress` fields (e.g. rtk-gains efficiency meter / impact column).
fn progress_bar(theme: &Theme, fraction: f64, accent: u32, height: f32) -> impl IntoElement {
    let f = fraction.clamp(0.0, 1.0) as f32;
    div()
        .h(px(height))
        .w_full()
        .bg(rgb(theme.colors.surface_hi))
        .rounded_md()
        .child(
            div()
                .h(px(height))
                .w(gpui::relative(f))
                .bg(rgb(accent))
                .rounded_md(),
        )
}

// ── More modal ───────────────────────────────────────────────────────────────

impl AuraView {
    fn render_more_modal(&self, cx: &mut Context<Self>) -> AnyElement {
        let lex = lexicon::pick(self.config.display.goblin_mode);
        // Append the running version so users can confirm the build they're
        // on without trawling stderr or `aura --version`.
        let updates_label = (lex.check_updates_fmt)(env!("CARGO_PKG_VERSION"));
        let items: [(&'static str, &'static str, String, ModalAction); 4] = [
            (
                "modal-updates",
                "icons/download.svg",
                updates_label,
                ModalAction::Updates,
            ),
            (
                "modal-sponsor",
                "icons/sparkle.svg",
                "Sponsor".to_string(),
                ModalAction::Sponsor,
            ),
            (
                "modal-github",
                "icons/github.svg",
                "View on GitHub".to_string(),
                ModalAction::Github,
            ),
            (
                "modal-issues",
                "icons/circle_help.svg",
                "Report issue".to_string(),
                ModalAction::Reports,
            ),
        ];

        // Backdrop covering the body — click to dismiss.
        let backdrop = div()
            .id("modal-backdrop")
            .absolute()
            .inset_0()
            .bg(rgba(0x000000a0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.close_more_modal(cx)));

        // Modal card itself. Clicks inside should not close the backdrop.
        let mut card = div()
            .id("modal-card")
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .min_w(px(260.0))
            .bg(rgb(self.theme.colors.surface))
            .rounded_md()
            .border_1()
            .border_color(rgb(self.theme.colors.border))
            // Swallow click so it doesn't bubble to the backdrop.
            .on_click(cx.listener(|_, _: &ClickEvent, _, _| {}));

        for (id, icon_path, label, action) in items {
            let surface_hi = self.theme.colors.surface_hi;
            card = card.child(
                div()
                    .id(SharedString::from(id))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .text_xs()
                    .text_color(rgb(self.theme.colors.text))
                    .hover(move |d| d.bg(rgb(surface_hi)))
                    .child(svg_icon(icon_path, self.theme.colors.text_dim, 14.0))
                    .child(SharedString::from(label))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.handle_modal_action(action, cx);
                    })),
            );
        }

        backdrop.child(card).into_any_element()
    }

    /// Render the Fleet tab: the account sanity line, one row per machine with
    /// 5h / weekly share bars and a freshness dot, then the pairing sub-panel.
    fn render_fleet(&self, accent: u32, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let mut col = div().flex().flex_col().px_4().py_3().gap_3();

        // ── Account sanity line ───────────────────────────────────────────────
        // Peers are read from the process-level manager's shared `FleetState`
        // (it runs whether or not this modal is open). `None` means the manager
        // has no running sync — fleet disabled or unpaired.
        let fleet_state = crate::runtime::fleet_state();
        let running = fleet_state.is_some();
        let (rows, account, reachable) = match &fleet_state {
            Some(state) => {
                // Recover from a poisoned lock rather than panicking the UI —
                // the data is still readable and a hostile broker must never
                // be able to take down the modal.
                let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                let now = Utc::now();
                (
                    guard.rows(now, self.config.fleet.stale_secs),
                    guard.account_pcts(),
                    guard.broker_reachable,
                )
            }
            None => (Vec::new(), None, true),
        };

        if let Some((session_pct, weekly_pct)) = account {
            col = col.child(
                div()
                    .flex()
                    .flex_row()
                    .gap_4()
                    .text_xs()
                    .text_color(rgb(theme.colors.text_dim))
                    .child(SharedString::from(format!("5h session: {session_pct:.0}%")))
                    .child(SharedString::from(format!("Weekly: {weekly_pct:.0}%"))),
            );
        }

        if !reachable {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.colors.border))
                    .bg(rgb(theme.colors.surface))
                    .text_xs()
                    .text_color(rgb(theme.colors.warning))
                    .child("Broker unreachable — retrying."),
            );
        }

        // ── Machine rows ──────────────────────────────────────────────────────
        if rows.is_empty() {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.colors.border))
                    .bg(rgb(theme.colors.surface))
                    .text_xs()
                    .text_color(rgb(theme.colors.text_dim))
                    .child(if running {
                        "Waiting for machines… pair another machine to compare."
                    } else {
                        "Fleet is not paired yet. Pair a machine to begin."
                    }),
            );
        } else {
            for row in &rows {
                col = col.child(render_fleet_row(theme, row, accent));
            }
        }

        // ── Pairing sub-panel ─────────────────────────────────────────────────
        col = col.child(self.render_fleet_pairing(cx));

        col.into_any_element()
    }

    /// The generate / join / leave controls plus the transient code display.
    fn render_fleet_pairing(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let surface_hi = theme.colors.surface_hi;
        // "Paired" tracks whether the process-level manager has a running sync.
        // After a pair/join the manager reconciles on its next poll tick (~150
        // ms), so the Leave button appears almost immediately.
        let paired = crate::runtime::fleet_state().is_some();

        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .mt_2()
            .pt_3()
            .border_t_1()
            .border_color(rgb(theme.colors.border));

        // Transient code: shown only right after generating, never persisted.
        if let Some(code) = &self.fleet_code {
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(theme.colors.surface))
                    .border_1()
                    .border_color(rgb(theme.colors.border))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.colors.text_dim))
                            .child("Pairing code (paste on the other machine):"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(theme.colors.text))
                            .child(SharedString::from(code.clone())),
                    )
                    .child(
                        fleet_button("fleet-copy-code", "Copy code", theme).on_click(
                            cx.listener(|view, _: &ClickEvent, _, cx| view.fleet_copy_code(cx)),
                        ),
                    ),
            );
        }

        // Status line.
        if let Some(status) = &self.fleet_status {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.colors.text_dim))
                    .child(SharedString::from(status.clone())),
            );
        }

        // Action row.
        let mut actions = div().flex().flex_row().flex_wrap().gap_2();
        actions = actions.child(
            fleet_button("fleet-pair", "Pair a machine", theme)
                .hover(move |d| d.bg(rgb(surface_hi)))
                .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.fleet_generate_code(cx))),
        );
        actions = actions.child(
            fleet_button("fleet-join", "Join from clipboard", theme)
                .hover(move |d| d.bg(rgb(surface_hi)))
                .on_click(
                    cx.listener(|view, _: &ClickEvent, _, cx| view.fleet_join_from_clipboard(cx)),
                ),
        );
        if paired {
            actions = actions.child(
                fleet_button("fleet-leave", "Leave fleet", theme)
                    .hover(move |d| d.bg(rgb(surface_hi)))
                    .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.fleet_leave(cx))),
            );
        }
        panel = panel.child(actions);

        panel.into_any_element()
    }

    fn render_settings_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let backdrop = div()
            .id("settings-backdrop")
            .absolute()
            .inset_0()
            .bg(gpui::rgba(0x000000a0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.close_settings_panel(cx)));

        let mut card = div()
            .id("settings-card")
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .min_w(gpui::px(260.0))
            .bg(rgb(self.theme.colors.surface))
            .rounded_md()
            .border_1()
            .border_color(rgb(self.theme.colors.border))
            .on_click(cx.listener(|_, _: &ClickEvent, _, _| {}));

        let text = self.theme.colors.text;
        let text_dim = self.theme.colors.text_dim;
        let surface_hi = self.theme.colors.surface_hi;
        let lex = lexicon::pick(self.config.display.goblin_mode);

        // ── Open config file ─────────────────────────────────────────────────
        card = card.child(
            div()
                .id("settings-open-config")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py_2()
                .rounded_md()
                .text_xs()
                .text_color(rgb(text))
                .hover(move |d| d.bg(rgb(surface_hi)))
                .child(svg_icon("icons/arrow_up_right.svg", text_dim, 14.0))
                .child(lex.menu_open_config)
                .on_click(cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.open_config(cx);
                    view.close_settings_panel(cx);
                })),
        );

        // ── Themes ───────────────────────────────────────────────────────────
        card = card.child(
            div()
                .id("settings-themes")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py_2()
                .rounded_md()
                .text_xs()
                .text_color(rgb(text))
                .hover(move |d| d.bg(rgb(surface_hi)))
                .child(svg_icon("icons/sliders.svg", text_dim, 14.0))
                .child(lex.menu_themes)
                .on_click(cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.open_theme(cx);
                    view.close_settings_panel(cx);
                })),
        );

        backdrop.child(card).into_any_element()
    }

    fn handle_modal_action(&mut self, action: ModalAction, cx: &mut Context<Self>) {
        match action {
            ModalAction::Updates => open_url(GITHUB_RELEASES_URL),
            ModalAction::Sponsor => open_url(SPONSOR_URL),
            ModalAction::Github => open_url(GITHUB_REPO_URL),
            ModalAction::Reports => open_url(GITHUB_ISSUES_URL),
        }
        self.close_more_modal(cx);
    }
}

#[derive(Debug, Clone, Copy)]
enum ModalAction {
    Updates,
    Sponsor,
    Github,
    Reports,
}

const GITHUB_REPO_URL: &str = "https://github.com/Rfluid/aura";
const GITHUB_RELEASES_URL: &str = "https://github.com/Rfluid/aura/releases";
const GITHUB_ISSUES_URL: &str = "https://github.com/Rfluid/aura/issues";
const SPONSOR_URL: &str = "https://github.com/Rfluid/aura/blob/main/SPONSOR.md";

fn open_url(url: &str) {
    crate::platform::open_url(url);
}

// ── Fleet helpers ─────────────────────────────────────────────────────────────

/// One machine row: a freshness dot, the label (with a "you" tag), and the
/// 5h + weekly share bars. Stale peers render dimmed.
fn render_fleet_row(theme: &Theme, row: &FleetRow, accent: u32) -> impl IntoElement {
    let label_color = if row.is_stale {
        theme.colors.text_dim
    } else {
        theme.colors.text
    };
    let dot_color = if row.is_stale {
        theme.colors.text_dim
    } else {
        accent
    };

    let label = if row.is_self {
        format!("{} (you)", row.label)
    } else {
        row.label.clone()
    };

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    // Freshness dot.
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(rgb(dot_color)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(label_color))
                        .child(SharedString::from(label)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .child(SharedString::from(fleet_freshness_label(row))),
                ),
        )
        .child(fleet_share_bar(theme, "5h", row.session_share, accent))
        .child(fleet_share_bar(theme, "wk", row.weekly_share, accent))
}

/// A single labelled share bar. `share` is a 0.0–1.0 fraction, or `None` when
/// the machine is stale / reported no tokens (rendered as a dashed em).
fn fleet_share_bar(theme: &Theme, tag: &str, share: Option<f64>, accent: u32) -> impl IntoElement {
    let pct_label = match share {
        Some(s) => format!("{:.0}%", s * 100.0),
        None => "—".to_string(),
    };
    let fraction = share.unwrap_or(0.0).clamp(0.0, 1.0) as f32;

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(20.0))
                .text_xs()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(tag.to_string())),
        )
        .child(
            div()
                .h(px(8.0))
                .flex_1()
                .bg(rgb(theme.colors.surface_hi))
                .rounded_md()
                .child(
                    div()
                        .h(px(8.0))
                        .w(gpui::relative(fraction))
                        .bg(rgb(accent))
                        .rounded_md(),
                ),
        )
        .child(
            div()
                .w(px(36.0))
                .text_xs()
                .text_color(rgb(theme.colors.text))
                .child(SharedString::from(pct_label)),
        )
}

/// "updated 8s ago" / "updated 3m ago" freshness string for a peer row.
fn fleet_freshness_label(row: &FleetRow) -> String {
    let secs = row.age_secs.max(0);
    if secs < 60 {
        format!("updated {secs}s ago")
    } else {
        format!("updated {}m ago", secs / 60)
    }
}

/// A small bordered text button used in the Fleet pairing sub-panel. Caller
/// chains `.on_click(...)` (and any `.hover(...)`).
fn fleet_button(id: &'static str, label: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .py_2()
        .rounded_md()
        .text_xs()
        .text_color(rgb(theme.colors.text))
        .bg(rgb(theme.colors.surface))
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(label)
}

/// The system hostname, used as the default Fleet machine label. Falls back to
/// `"this machine"` when the env vars aren't set (rare).
pub fn hostname_label() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .filter(|h| !h.trim().is_empty())
        .unwrap_or_else(|| "this machine".to_string())
}

/// Config directory the Fleet heartbeat reads Claude quota from: the first
/// configured Claude Code agent's resolved path, or the default `~/.claude` when
/// none is configured. Returned by value so the result can move into the
/// `Send + 'static` heartbeat-source closure. Shared by the modal view and the
/// process-level Fleet manager so both read the same path.
pub fn fleet_claude_config_path(config: &AppConfig) -> PathBuf {
    config
        .agents
        .iter()
        .find(|a| a.kind == AgentKind::ClaudeCode)
        .map(|a| a.resolved_config_path())
        .unwrap_or_else(|| {
            AgentConfig {
                name: String::new(),
                kind: AgentKind::ClaudeCode,
                config_path: None,
                color: None,
            }
            .resolved_config_path()
        })
}

/// This machine's display label: the configured `[fleet].machine_label`
/// override, or the system hostname when blank. Shared by the modal view and
/// the process-level Fleet manager.
pub fn fleet_machine_label(config: &AppConfig) -> String {
    let configured = config.fleet.machine_label.trim();
    if !configured.is_empty() {
        return configured.to_string();
    }
    hostname_label()
}

// ── Activity (live Claude Code process monitor) ─────────────────────────────────

/// Render the live Activity tab: one card per Claude Code session (root +
/// subtree) sorted by total CPU%, each with its heaviest child processes, plus
/// a footer of account-wide totals. `primed` is false on the very first sample
/// (no CPU baseline yet) — in that case CPU numbers render as "measuring…".
fn render_activity(
    theme: &Theme,
    sessions: &[ClaudeSession],
    primed: bool,
    refresh_secs: u64,
    accent: u32,
) -> AnyElement {
    let mut col = div().flex().flex_col().px_4().py_3().gap_3();

    // ── Header: "ACTIVITY · live    ⟳ 3s" ─────────────────────────────────────
    col = col.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.colors.text_dim))
                    .child("ACTIVITY · live"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(theme.colors.text_dim))
                    .child(SharedString::from(format!("⟳ {refresh_secs}s"))),
            ),
    );

    if sessions.is_empty() {
        return col
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(theme.colors.border))
                    .bg(rgb(theme.colors.surface))
                    .text_xs()
                    .text_color(rgb(theme.colors.text_dim))
                    .child("No Claude Code processes running."),
            )
            .into_any_element();
    }

    for session in sessions {
        col = col.child(render_activity_session(theme, session, primed, accent));
    }

    // ── Footer: total CPU% · RAM · session count ──────────────────────────────
    let total_cpu: f32 = sessions.iter().map(|s| s.total_cpu).sum();
    let total_mem: u64 = sessions.iter().map(|s| s.total_mem_bytes).sum();
    let count = sessions.len();
    let cpu_label = if primed {
        format!("{total_cpu:.0}% CPU")
    } else {
        "measuring…".to_string()
    };
    let session_word = if count == 1 { "session" } else { "sessions" };
    col = col.child(
        div()
            .pt_2()
            .border_t_1()
            .border_color(rgb(theme.colors.border))
            .text_xs()
            .text_color(rgb(theme.colors.text))
            .child(SharedString::from(format!(
                "Total Claude Code: {cpu_label} · {} · {count} {session_word}",
                format_mem_bytes(total_mem),
            ))),
    );

    col.into_any_element()
}

/// One session card: header row (status dot · project · session · totals) plus
/// the heaviest child rows beneath it.
fn render_activity_session(
    theme: &Theme,
    session: &ClaudeSession,
    primed: bool,
    accent: u32,
) -> impl IntoElement {
    let title = match &session.session_id {
        Some(id) => format!("{} · {id}…", session.project),
        None => session.project.clone(),
    };
    let cpu_mem = if primed {
        format!(
            "{:.0}% CPU · {}",
            session.total_cpu,
            format_mem_bytes(session.total_mem_bytes)
        )
    } else {
        format!("measuring… · {}", format_mem_bytes(session.total_mem_bytes))
    };

    let mut card = div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .bg(rgb(theme.colors.surface))
        .rounded_md()
        .border_1()
        .border_color(rgb(theme.colors.border))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    // Live-session dot, accent-coloured.
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .bg(rgb(accent)),
                )
                .child(
                    div()
                        .flex_1()
                        .text_color(rgb(theme.colors.text))
                        .child(SharedString::from(title)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(theme.colors.text_dim))
                        .child(SharedString::from(cpu_mem)),
                ),
        );

    for child in &session.children {
        card = card.child(render_activity_child(theme, child, primed));
    }
    card
}

/// One culprit child row: "↳ node mcp-server-figma    180% · 0.8 GB".
fn render_activity_child(theme: &Theme, child: &ProcView, primed: bool) -> impl IntoElement {
    let metrics = if primed {
        format!("{:.0}% · {}", child.cpu, format_mem_bytes(child.mem_bytes))
    } else {
        format!("measuring… · {}", format_mem_bytes(child.mem_bytes))
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .pl_3()
        .text_xs()
        .child(
            div()
                .flex_1()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(format!("↳ {}", child.label))),
        )
        .child(
            div()
                .text_color(rgb(theme.colors.text_dim))
                .child(SharedString::from(metrics)),
        )
}

/// Human-readable RAM: bytes → "812 MB" / "2.1 GB". Uses binary units (GiB/MiB)
/// under the conventional GB/MB labels, matching how Activity Monitor and
/// `top` report process RSS.
fn format_mem_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

// ── Helpers: icons, buttons, color math ──────────────────────────────────────

fn svg_icon(path: &'static str, color: u32, size: f32) -> impl IntoElement {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// Same as `svg_icon` but accepts a `SharedString` so callers can pass an
/// owned path (e.g. one resolved from plugin config at runtime).
fn svg_icon_dynamic(path: SharedString, color: u32, size: f32) -> impl IntoElement {
    svg()
        .path(path)
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// Resolve the icon path for a plugin. Returns the configured `icon` field
/// when present, otherwise a sensible default per known plugin command (so
/// first-party plugins keep their brand glyph even in pre-`icon`-field
/// configs), falling back to the generic `blocks` icon. Path resolution
/// (asset name vs. absolute vs. relative-to-config) happens in the asset
/// loader.
fn plugin_icon_path(plugin: &PluginConfig) -> SharedString {
    if let Some(icon) = plugin.icon.clone() {
        return SharedString::from(icon);
    }
    match plugin.command.as_str() {
        "aura-plugin-rtk" => SharedString::from("icons/rtk.svg"),
        _ => SharedString::from("icons/blocks.svg"),
    }
}

/// A click-target wrapping an SVG icon. Caller chains `.on_click(...)`.
fn icon_button(id: &'static str, path: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let text = theme.colors.text;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(20.0))
        .h(px(20.0))
        .text_color(rgb(theme.colors.text_dim))
        .hover(move |d| d.text_color(rgb(text)))
        .child(svg_icon(path, theme.colors.text_dim, 14.0))
}

/// Icon for the given agent profile, tinted with the agent's brand color.
fn agent_icon(agent: &AgentConfig, theme: &Theme) -> impl IntoElement {
    let path = match agent.kind {
        AgentKind::ClaudeCode => "icons/claude.svg",
        AgentKind::Codex => "icons/openai.svg",
        AgentKind::Gemini => "icons/gemini.svg",
    };
    svg()
        .path(path)
        .size(px(14.0))
        .flex_none()
        .text_color(rgb(theme.agent_accent(agent)))
}

fn rgba(value: u32) -> gpui::Rgba {
    let r = ((value >> 24) & 0xff) as f32 / 255.0;
    let g = ((value >> 16) & 0xff) as f32 / 255.0;
    let b = ((value >> 8) & 0xff) as f32 / 255.0;
    let a = (value & 0xff) as f32 / 255.0;
    gpui::Rgba { r, g, b, a }
}

// `work_area` lives at `crate::work_area`; both this module and `main.rs`
// use it. See `work_area.rs` for the parsing rationale.

/// Pure form of `AuraView::show_update_button`. Returns true when there
/// is fetched update info, the user hasn't muted all update prompts, and
/// they haven't already dismissed *this exact* version.
fn should_show_update_button(
    update: Option<&UpdateInfo>,
    cfg: &aura_core::config::UpdateConfig,
) -> bool {
    let Some(info) = update else {
        return false;
    };
    if cfg.dismiss_all {
        return false;
    }
    !matches!(&cfg.dismissed_version, Some(v) if *v == info.latest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::config::UpdateConfig;
    use semver::Version;

    fn info(v: &str) -> UpdateInfo {
        UpdateInfo {
            latest: Version::parse(v).unwrap(),
        }
    }

    #[test]
    fn hides_when_no_update_info() {
        let cfg = UpdateConfig::default();
        assert!(!should_show_update_button(None, &cfg));
    }

    #[test]
    fn hides_when_dismiss_all_set() {
        let cfg = UpdateConfig {
            dismissed_version: None,
            dismiss_all: true,
        };
        assert!(!should_show_update_button(Some(&info("0.1.18")), &cfg));
    }

    #[test]
    fn hides_when_dismissed_version_matches_latest() {
        let cfg = UpdateConfig {
            dismissed_version: Some("0.1.18".into()),
            dismiss_all: false,
        };
        assert!(!should_show_update_button(Some(&info("0.1.18")), &cfg));
    }

    #[test]
    fn shows_when_dismissed_older_release_and_newer_is_out() {
        let cfg = UpdateConfig {
            dismissed_version: Some("0.1.18".into()),
            dismiss_all: false,
        };
        assert!(should_show_update_button(Some(&info("0.1.19")), &cfg));
    }

    #[test]
    fn shows_when_never_dismissed() {
        let cfg = UpdateConfig::default();
        assert!(should_show_update_button(Some(&info("0.1.18")), &cfg));
    }
}
