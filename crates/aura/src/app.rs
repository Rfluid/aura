use std::{path::PathBuf, time::Duration};

use aura_core::{
    config::{parse_hex_color, AgentConfig, AgentKind, AppConfig, PluginConfig},
    plugin::{PluginContent, PluginPanel, PluginRunner, PluginSection},
    quota::{CodexQuota, QuotaApi, QuotaSnapshot, QuotaSource, QuotaWindow},
    reader::{make_reader, Period, UsageSnapshot},
    state::AppState,
};
use chrono::{DateTime, Local, Timelike, Utc};
use gpui::{div, prelude::*, px, rgb, svg, AnyElement, ClickEvent, Context, SharedString, Window};

use crate::format::{duration, hour_of_day, locale_uses_12h, system_locale, thousands};

// ── Theme tokens (Zed-ish dark) ───────────────────────────────────────────────
const COLOR_BG: u32 = 0x0e0e10;
const COLOR_SURFACE: u32 = 0x1a1a1f;
const COLOR_SURFACE_HI: u32 = 0x252530;
const COLOR_BORDER: u32 = 0x2d2d36;
const COLOR_TEXT: u32 = 0xe6e6ee;
const COLOR_TEXT_DIM: u32 = 0x8a8a9a;
const COLOR_ACCENT: u32 = 0x8b5cf6;
const COLOR_WARNING: u32 = 0xe0a96d;

// Per-agent brand tints used for the icon next to each profile name.
const COLOR_CLAUDE: u32 = 0xd97757;
const COLOR_OPENAI: u32 = 0xffffff;

/// Maximum height of the body region in pixels. The window itself is 640px
/// tall and the header / selector / period / tab rows together use ~200px,
/// so 440px is roughly the most the body can claim before the window starts
/// to feel cramped. When the body's natural height exceeds this, the
/// wrapping container becomes vertically scrollable.
const MAX_BODY_HEIGHT: f32 = 440.0;

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
    Summary,
    Models,
}

impl AgentSection {
    fn label(self) -> &'static str {
        match self {
            Self::Quota => "Quota",
            Self::Summary => "Summary",
            Self::Models => "Models",
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Summary => "summary",
            Self::Models => "models",
        }
    }

    /// Whether this section filters data by the active period. Quota
    /// reports rolling 5h / 7d subscription windows fixed by the API, so
    /// the period pills don't apply.
    fn uses_period(self) -> bool {
        match self {
            Self::Quota => false,
            Self::Summary | Self::Models => true,
        }
    }
}

pub struct AuraView {
    config: AppConfig,
    config_path: PathBuf,
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
    /// Indexed by plugin name.
    plugin_panels: Vec<(String, PluginPanel)>,

    show_more_modal: bool,
    is_loading: bool,
    /// Index into `SPINNER_FRAMES`, advanced by a timer while `is_loading`.
    spinner_frame: usize,
    error: Option<String>,
}

/// Bundle of results produced by the background refresh task. Errors that
/// previously landed on `self.error` are bubbled up here so they survive
/// the trip across threads.
struct RefreshResult {
    /// Reloaded config — `None` means "keep the old one".
    config: Option<AppConfig>,
    /// `Some` if the active profile had to fall back to the first agent.
    fallback_profile: Option<String>,
    snapshot: Option<UsageSnapshot>,
    quota: Option<QuotaSnapshot>,
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

        let mut view = Self {
            config,
            config_path,
            state,
            active_profile,
            active_period,
            mode: Mode::Agent,
            active_plugin: None,
            active_agent_section: AgentSection::Quota,
            active_plugin_section: None,
            snapshot: None,
            quota: None,
            plugin_panels: Vec::new(),
            show_more_modal: false,
            is_loading: false,
            spinner_frame: 0,
            error: None,
        };
        // Initial load: kick off the async refresh now so the spinner can
        // render on first paint instead of blocking construction.
        view.refresh(cx);
        view
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
        let active_profile = self.active_profile.clone();
        let period = self.active_period;

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { do_refresh(config_path, active_profile, period) })
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
            self.config = cfg;
        }
        if let Some(fallback) = result.fallback_profile {
            self.active_profile = fallback;
        }
        self.snapshot = result.snapshot;
        self.quota = result.quota;
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
fn do_refresh(config_path: PathBuf, active_profile: String, period: Period) -> RefreshResult {
    // Reload config so edits made via the settings button take effect.
    let config = match AppConfig::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            return RefreshResult {
                config: None,
                fallback_profile: None,
                snapshot: None,
                quota: None,
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
            fallback_profile,
            snapshot: None,
            quota: None,
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
    let quota = Some(match agent.kind {
        AgentKind::ClaudeCode => QuotaApi::new(agent_path).snapshot(),
        AgentKind::Codex => CodexQuota::new(agent_path).snapshot(),
    });

    // Plugins: each runs as a subprocess with `--period` passed through.
    let plugin_panels = config
        .plugins
        .iter()
        .map(|p| (p.name.clone(), PluginRunner::run_with_period(p, period)))
        .collect();

    RefreshResult {
        config: Some(config),
        fallback_profile,
        snapshot,
        quota,
        plugin_panels,
        error,
    }
}

impl AuraView {
    /// Open the config file in the desktop's default editor.
    /// Tries `xdg-open` first (right call from a GUI context), falls back to
    /// `$EDITOR` if it's set.
    fn open_config(&mut self, cx: &mut Context<Self>) {
        use std::process::{Command, Stdio};

        // Ensure the file exists so the editor doesn't open a blank buffer.
        if !self.config_path.exists() {
            if let Err(e) = AppConfig::load(&self.config_path) {
                self.error = Some(format!("Could not create config: {e}"));
                cx.notify();
                return;
            }
        }

        let spawned = Command::new("xdg-open")
            .arg(&self.config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();

        if spawned.is_err() {
            if let Ok(editor) = std::env::var("EDITOR") {
                let _ = Command::new(editor).arg(&self.config_path).spawn();
            } else {
                self.error = Some(format!(
                    "Could not open editor (no `xdg-open` and `$EDITOR` unset). \
                     Edit {} manually.",
                    self.config_path.display()
                ));
                cx.notify();
            }
        }
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
            cx.notify();
        }
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

    fn toggle_more_modal(&mut self, cx: &mut Context<Self>) {
        self.show_more_modal = !self.show_more_modal;
        cx.notify();
    }

    fn close_more_modal(&mut self, cx: &mut Context<Self>) {
        self.show_more_modal = false;
        cx.notify();
    }

    fn current_agent(&self) -> Option<&AgentConfig> {
        self.config
            .agents
            .iter()
            .find(|a| a.name == self.active_profile)
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
                .map(agent_accent)
                .unwrap_or(COLOR_ACCENT),
            Mode::Plugin => self
                .current_plugin_config()
                .map(plugin_accent)
                .unwrap_or(COLOR_ACCENT),
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AuraView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(COLOR_BG))
            .text_color(rgb(COLOR_TEXT))
            .font_family("monospace")
            .text_sm()
            .child(self.render_header(cx))
            .child(self.render_selector_row(cx))
            .when(self.current_section_uses_period(), |d| {
                d.child(self.render_period_row(cx))
            })
            .child(self.render_tab_row(cx))
            .child(self.render_body(cx));

        if self.show_more_modal {
            root = root.child(self.render_more_modal(cx));
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
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER))
            .bg(rgb(COLOR_SURFACE));

        // Left: brand (+ spinner when a fetch is in flight)
        let frame = SPINNER_FRAMES[self.spinner_frame % SPINNER_FRAMES.len()];
        let brand = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(svg_icon("icons/aura.svg", COLOR_ACCENT, 18.0))
            .when(self.is_loading, |d| {
                d.child(div().text_color(rgb(COLOR_TEXT_DIM)).child(frame))
            });

        // Right: action buttons (refresh, config, more)
        let actions =
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    icon_button("act-refresh", "icons/rotate_cw.svg").on_click(cx.listener(
                        |view, _: &ClickEvent, _, cx| {
                            // Early-return if a refresh is already running so we
                            // don't double-spawn.
                            if view.is_loading {
                                return;
                            }
                            view.refresh(cx);
                        },
                    )),
                )
                .child(
                    icon_button("act-config", "icons/settings.svg")
                        .on_click(cx.listener(|view, _: &ClickEvent, _, cx| view.open_config(cx))),
                )
                .child(icon_button("act-more", "icons/ellipsis.svg").on_click(
                    cx.listener(|view, _: &ClickEvent, _, cx| view.toggle_more_modal(cx)),
                ));

        row.child(brand).child(actions).into_any_element()
    }

    /// Pill row + agent/plugin mode toggle.
    fn render_selector_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER));

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
        let mut picker = div().flex().flex_row().gap_2();
        for agent in &self.config.agents {
            let name = agent.name.clone();
            let active = self.active_profile == agent.name;
            let pill_name = name.clone();
            let accent = agent_accent(agent);
            let icon = agent_icon(agent);
            picker = picker.child(
                div()
                    .id(SharedString::from(format!("profile-{}", agent.name)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(active, |d| {
                        d.bg(rgb(blend(accent, COLOR_BG, 0.75)))
                            .text_color(rgb(COLOR_TEXT))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(COLOR_SURFACE_HI)).text_color(rgb(COLOR_TEXT_DIM))
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
            return div()
                .text_xs()
                .text_color(rgb(COLOR_TEXT_DIM))
                .child("No plugins configured")
                .into_any_element();
        }
        let mut picker = div().flex().flex_row().gap_2();
        for plugin in &self.config.plugins {
            let name = plugin.name.clone();
            let active = self.active_plugin.as_deref() == Some(name.as_str());
            let pill_name = name.clone();
            let accent = plugin_accent(plugin);
            let icon_path = plugin_icon_path(plugin);
            let icon_color = if active { accent } else { COLOR_TEXT_DIM };
            picker = picker.child(
                div()
                    .id(SharedString::from(format!("plugin-{}", plugin.name)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(active, |d| {
                        d.bg(rgb(blend(accent, COLOR_BG, 0.75)))
                            .text_color(rgb(COLOR_TEXT))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(COLOR_SURFACE_HI)).text_color(rgb(COLOR_TEXT_DIM))
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
        let modes = [
            ("Agents", Mode::Agent, "mode-agent"),
            ("Plugins", Mode::Plugin, "mode-plugin"),
        ];
        let mut row = div().flex().flex_row().gap_1();
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
                        d.bg(rgb(COLOR_ACCENT)).text_color(rgb(0xffffff))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(COLOR_SURFACE)).text_color(rgb(COLOR_TEXT_DIM))
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
        let periods = [
            ("All time", Period::AllTime, "period-all"),
            ("Last 7 days", Period::Last7Days, "period-7"),
            ("Last 30 days", Period::Last30Days, "period-30"),
        ];

        // Selected filter background uses the active agent's (or plugin's)
        // accent color so the filter row reads as "belonging to" the
        // currently-selected profile — see .design/agents.md.
        let accent = self.current_accent();
        let on_accent = on_accent_text(accent);

        let mut row = div()
            .flex()
            .flex_row()
            .gap_2()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER));

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
                        d.bg(rgb(COLOR_SURFACE)).text_color(rgb(COLOR_TEXT_DIM))
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
            .flex()
            .flex_row()
            .gap_4()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER))
            .bg(rgb(COLOR_SURFACE));

        match self.mode {
            Mode::Agent => {
                let sections = [
                    AgentSection::Quota,
                    AgentSection::Summary,
                    AgentSection::Models,
                ];
                for s in sections {
                    let active = self.active_agent_section == s;
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("agent-section-{}", s.id())))
                            .text_sm()
                            .pb_1()
                            .when(active, |d| {
                                d.text_color(rgb(COLOR_TEXT))
                                    .border_b_2()
                                    .border_color(rgb(accent))
                            })
                            .when(!active, |d| d.text_color(rgb(COLOR_TEXT_DIM)))
                            .child(s.label())
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
                                .text_sm()
                                .pb_1()
                                .when(active, |d| {
                                    d.text_color(rgb(COLOR_TEXT))
                                        .border_b_2()
                                        .border_color(rgb(accent))
                                })
                                .when(!active, |d| d.text_color(rgb(COLOR_TEXT_DIM)))
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

    fn render_body(&self, _cx: &mut Context<Self>) -> AnyElement {
        let inner: AnyElement = if let Some(err) = &self.error {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .p_6()
                .child(div().text_color(rgb(0xff6b6b)).child(err.clone()))
                .into_any_element()
        } else {
            let accent = self.current_accent();
            match self.mode {
                Mode::Agent => match self.active_agent_section {
                    AgentSection::Quota => render_quota(self.quota.as_ref(), accent),
                    AgentSection::Summary => match self.snapshot.as_ref() {
                        Some(snap) => render_summary(snap),
                        None => render_loading(),
                    },
                    AgentSection::Models => match self.snapshot.as_ref() {
                        Some(snap) => render_models(snap, accent),
                        None => render_loading(),
                    },
                },
                Mode::Plugin => self.render_plugin_body(),
            }
        };

        div()
            .id("body-scroll")
            .max_h(px(MAX_BODY_HEIGHT))
            .overflow_y_scroll()
            .child(inner)
            .into_any_element()
    }

    fn render_plugin_body(&self) -> AnyElement {
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
                        .text_color(rgb(COLOR_TEXT_DIM))
                        .child("No plugin selected"),
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
                        .text_color(rgb(0xff6b6b))
                        .child(SharedString::from(err.clone())),
                )
                .into_any_element();
        }

        let section_id = self.active_plugin_section.as_deref().unwrap_or("");
        let section = panel.section(section_id).or_else(|| panel.sections.first());
        match section {
            Some(s) => render_plugin_section(s, self.current_accent()),
            None => render_loading(),
        }
    }
}

// ── Quota (subscription windows — mirrors `claude /usage`) ───────────────────

fn render_loading() -> AnyElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .child(div().text_color(rgb(COLOR_TEXT_DIM)).child("Loading…"))
        .into_any_element()
}

fn render_quota(quota: Option<&QuotaSnapshot>, accent: u32) -> AnyElement {
    let Some(quota) = quota else {
        return render_loading();
    };

    let mut col = div().flex().flex_col().px_4().py_3().gap_3();

    // Show the subscription tier (e.g. "pro", "max") when the API was reached.
    if let Some(sub) = &quota.subscription_type {
        col = col.child(
            div()
                .text_xs()
                .text_color(rgb(COLOR_TEXT_DIM))
                .child(SharedString::from(format!("Subscription: {sub}"))),
        );
    }

    if quota.source != QuotaSource::Api {
        col = col.child(render_fallback_warning(quota));
    }

    if quota.windows.is_empty() {
        if quota.source == QuotaSource::Api {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(COLOR_BORDER))
                    .bg(rgb(COLOR_SURFACE))
                    .text_color(rgb(COLOR_TEXT_DIM))
                    .text_xs()
                    .child("No quota data available."),
            );
        }
    } else {
        for w in &quota.windows {
            col = col.child(render_quota_window(w, accent));
        }
    }

    col.into_any_element()
}

/// Discrete warning chip shown when the quota snapshot didn't come from the
/// `/api/oauth/usage` endpoint. Distinguishes the fallback kind so the user
/// knows whether numbers are local estimates or absent entirely.
fn render_fallback_warning(quota: &QuotaSnapshot) -> AnyElement {
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
        .border_color(rgb(COLOR_BORDER))
        .bg(rgb(COLOR_SURFACE))
        .child(
            div()
                .mt(px(1.0))
                .child(svg_icon("icons/info.svg", COLOR_WARNING, 12.0)),
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
                        .text_color(rgb(COLOR_WARNING))
                        .child(SharedString::from(kind)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(COLOR_TEXT_DIM))
                        .child(SharedString::from(note)),
                ),
        )
        .into_any_element()
}

fn render_quota_window(w: &QuotaWindow, accent: u32) -> impl IntoElement {
    // Percent used: either real (from API) or unknown (fallback).
    let pct = w.used_percentage.unwrap_or(0.0).clamp(0.0, 100.0);
    let pct_label = match w.used_percentage {
        Some(p) => format!("{:.0}% used", p),
        None => match w.used_tokens {
            Some(t) => format!("{} tokens", thousands(t)),
            None => "—".to_string(),
        },
    };

    let resets_label = w
        .resets_at
        .map(format_reset)
        .unwrap_or_else(|| "—".to_string());

    let bar_fraction = (pct / 100.0) as f32;

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_3()
        .bg(rgb(COLOR_SURFACE))
        .rounded_md()
        .border_1()
        .border_color(rgb(COLOR_BORDER))
        .child(
            div()
                .text_color(rgb(COLOR_TEXT))
                .child(SharedString::from(w.label.clone())),
        )
        .child(
            // Bar
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .h(px(8.0))
                        .flex_1()
                        .bg(rgb(COLOR_SURFACE_HI))
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
                        .text_color(rgb(COLOR_TEXT))
                        .child(SharedString::from(pct_label)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(COLOR_TEXT_DIM))
                .child(SharedString::from(format!("Resets {}", resets_label))),
        )
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

// ── Summary (the old "Overview" — stat-card grid) ────────────────────────────

fn render_summary(snap: &UsageSnapshot) -> AnyElement {
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
            row = row.child(stat_card(label, value));
        }
        col = col.child(row);
    }
    col.into_any_element()
}

fn stat_card(label: &str, value: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .gap_1()
        .px_3()
        .py_2()
        .bg(rgb(COLOR_SURFACE))
        .rounded_md()
        .border_1()
        .border_color(rgb(COLOR_BORDER))
        .child(
            div()
                .text_xs()
                .text_color(rgb(COLOR_TEXT_DIM))
                .child(SharedString::from(label.to_string())),
        )
        .child(
            div()
                .text_color(rgb(COLOR_TEXT))
                .child(SharedString::from(value.to_string())),
        )
}

// ── Models ────────────────────────────────────────────────────────────────────

fn render_models(snap: &UsageSnapshot, accent: u32) -> AnyElement {
    let mut col = div().flex().flex_col().px_4().py_3().gap_4();

    // Tokens per Day chart
    col = col.child(render_daily_chart(snap, accent));

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
        models_col = models_col.child(render_model_row(&m.model, tokens, pct, accent));
    }
    col = col.child(models_col);
    col.into_any_element()
}

fn render_model_row(model: &str, tokens: u64, pct: f64, accent: u32) -> impl IntoElement {
    let bar_width_pct = pct.clamp(2.0, 100.0);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .bg(rgb(COLOR_SURFACE))
        .rounded_md()
        .border_1()
        .border_color(rgb(COLOR_BORDER))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .child(
                    div()
                        .text_color(rgb(COLOR_TEXT))
                        .child(SharedString::from(model.to_string())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(COLOR_TEXT_DIM))
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
                .bg(rgb(COLOR_SURFACE_HI))
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

fn render_daily_chart(snap: &UsageSnapshot, accent: u32) -> impl IntoElement {
    let days: Vec<(String, u64)> = snap
        .daily_tokens
        .iter()
        .map(|d| (d.date.clone(), d.by_model.values().sum::<u64>()))
        .collect();

    let max = days.iter().map(|(_, n)| *n).max().unwrap_or(0);

    let mut bars = div().flex().flex_row().items_end().gap_1().h(px(56.0));
    for (_, n) in &days {
        let height = if max > 0 {
            (*n as f32 / max as f32 * 48.0).max(2.0)
        } else {
            2.0
        };
        bars = bars.child(div().flex_1().h(px(height)).bg(rgb(accent)).rounded_sm());
    }

    div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .bg(rgb(COLOR_SURFACE))
        .rounded_md()
        .border_1()
        .border_color(rgb(COLOR_BORDER))
        .child(
            div()
                .text_xs()
                .text_color(rgb(COLOR_TEXT_DIM))
                .child("Tokens per day"),
        )
        .child(bars)
}

// ── Plugin section rendering ─────────────────────────────────────────────────

fn render_plugin_section(section: &PluginSection, accent: u32) -> AnyElement {
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
                    .bg(rgb(COLOR_SURFACE))
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(COLOR_BORDER));

                card = card.child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(COLOR_TEXT_DIM))
                                .child(SharedString::from(line.label.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .when(line.highlight, |d| d.text_color(rgb(accent)))
                                .when(!line.highlight, |d| d.text_color(rgb(COLOR_TEXT)))
                                .child(SharedString::from(line.value.clone())),
                        ),
                );

                if let Some(p) = line.progress {
                    card = card.child(progress_bar(p, accent, 6.0));
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
                .border_color(rgb(COLOR_BORDER));
            for (i, h) in headers.iter().enumerate() {
                let right = col_right.get(i).copied().unwrap_or(false);
                header_row = header_row.child(
                    div()
                        .w(px(col_widths[i]))
                        .flex_none()
                        .text_xs()
                        .text_color(rgb(COLOR_TEXT_DIM))
                        .when(right, |d| d.text_right())
                        .child(SharedString::from(h.clone())),
                );
            }
            if has_progress {
                header_row = header_row.child(
                    div()
                        .flex_1()
                        .text_xs()
                        .text_color(rgb(COLOR_TEXT_DIM))
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
                            .when(!row.highlight, |d| d.text_color(rgb(COLOR_TEXT)))
                            .when(right, |d| d.text_right())
                            .child(SharedString::from(cell.clone())),
                    );
                }
                if has_progress {
                    r = r.child(div().flex_1().child(progress_bar(
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
            .text_color(rgb(COLOR_TEXT))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
    }
}

/// A thin filled bar. `fraction` is clamped to 0.0–1.0. Used by plugin
/// `progress` fields (e.g. rtk-gains efficiency meter / impact column).
fn progress_bar(fraction: f64, accent: u32, height: f32) -> impl IntoElement {
    let f = fraction.clamp(0.0, 1.0) as f32;
    div()
        .h(px(height))
        .w_full()
        .bg(rgb(COLOR_SURFACE_HI))
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
        let items = [
            (
                "modal-themes",
                "icons/sliders.svg",
                "Themes",
                ModalAction::Themes,
            ),
            (
                "modal-updates",
                "icons/download.svg",
                "Check updates",
                ModalAction::Updates,
            ),
            (
                "modal-sponsor",
                "icons/sparkle.svg",
                "Sponsor",
                ModalAction::Sponsor,
            ),
            (
                "modal-issues",
                "icons/circle_help.svg",
                "Report issue",
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
            .bg(rgb(COLOR_SURFACE))
            .rounded_md()
            .border_1()
            .border_color(rgb(COLOR_BORDER))
            // Swallow click so it doesn't bubble to the backdrop.
            .on_click(cx.listener(|_, _: &ClickEvent, _, _| {}));

        for (id, icon_path, label, action) in items {
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
                    .text_color(rgb(COLOR_TEXT))
                    .hover(|d| d.bg(rgb(COLOR_SURFACE_HI)))
                    .child(svg_icon(icon_path, COLOR_TEXT_DIM, 14.0))
                    .child(label)
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.handle_modal_action(action, cx);
                    })),
            );
        }

        backdrop.child(card).into_any_element()
    }

    fn handle_modal_action(&mut self, action: ModalAction, cx: &mut Context<Self>) {
        match action {
            ModalAction::Themes => {
                // User-customizable themes are spec'd in .design/customization.md
                // but not implemented yet. Showing a placeholder.
                self.error = Some(
                    "Themes are coming soon. See .design/customization.md for the spec."
                        .to_string(),
                );
            }
            ModalAction::Updates => open_url(GITHUB_RELEASES_URL),
            ModalAction::Sponsor => open_url(SPONSOR_URL),
            ModalAction::Reports => open_url(GITHUB_ISSUES_URL),
        }
        self.close_more_modal(cx);
    }
}

#[derive(Debug, Clone, Copy)]
enum ModalAction {
    Themes,
    Updates,
    Sponsor,
    Reports,
}

const GITHUB_RELEASES_URL: &str = "https://github.com/Rfluid/aura/releases";
const GITHUB_ISSUES_URL: &str = "https://github.com/Rfluid/aura/issues";
const SPONSOR_URL: &str = "https://github.com/Rfluid/aura/blob/main/SPONSOR.md";

fn open_url(url: &str) {
    use std::process::{Command, Stdio};
    let _ = Command::new("xdg-open")
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
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
fn icon_button(id: &'static str, path: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(20.0))
        .h(px(20.0))
        .text_color(rgb(COLOR_TEXT_DIM))
        .hover(|d| d.text_color(rgb(COLOR_TEXT)))
        .child(svg_icon(path, COLOR_TEXT_DIM, 14.0))
}

/// Icon for the given agent profile, tinted with the agent's brand color.
fn agent_icon(agent: &AgentConfig) -> impl IntoElement {
    let path = match agent.kind {
        AgentKind::ClaudeCode => "icons/claude.svg",
        AgentKind::Codex => "icons/openai.svg",
    };
    svg()
        .path(path)
        .size(px(14.0))
        .flex_none()
        .text_color(rgb(agent_accent(agent)))
}

/// Default accent color for an agent kind. Spec: see `.design/agents.md`.
pub fn agent_kind_default_color(kind: AgentKind) -> u32 {
    match kind {
        AgentKind::ClaudeCode => COLOR_CLAUDE,
        AgentKind::Codex => COLOR_OPENAI,
    }
}

/// Resolve the accent color for a configured agent. Precedence:
/// 1. `config.color` (hex string from `~/.config/aura/config.toml`)
/// 2. Per-kind default (`COLOR_CLAUDE`, `COLOR_OPENAI`, …)
///
/// The luminance fallback applies last — any color (override or default) whose
/// relative luminance exceeds 0.85 is replaced with the neutral light grey
/// fallback so it reads against `COLOR_BG`. See `.design/agents.md`.
pub fn agent_accent(agent: &AgentConfig) -> u32 {
    let resolved = agent
        .color
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or_else(|| agent_kind_default_color(agent.kind));
    if relative_luminance(resolved) > 0.85 {
        COLOR_AGENT_FALLBACK
    } else {
        resolved
    }
}

/// Resolve the accent color for a configured plugin. Falls back to the global
/// `COLOR_ACCENT` when no override is set. The same luminance fallback as for
/// agents applies.
pub fn plugin_accent(plugin: &PluginConfig) -> u32 {
    let resolved = plugin
        .color
        .as_deref()
        .and_then(parse_hex_color)
        .unwrap_or(COLOR_ACCENT);
    if relative_luminance(resolved) > 0.85 {
        COLOR_AGENT_FALLBACK
    } else {
        resolved
    }
}

/// WCAG relative luminance, 0.0–1.0.
fn relative_luminance(rgb_hex: u32) -> f64 {
    let r = ((rgb_hex >> 16) & 0xff) as f64 / 255.0;
    let g = ((rgb_hex >> 8) & 0xff) as f64 / 255.0;
    let b = (rgb_hex & 0xff) as f64 / 255.0;
    let lin = |c: f64| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// Blend `a` toward `b` by `t` (0.0 = pure a, 1.0 = pure b).
fn blend(a: u32, b: u32, t: f64) -> u32 {
    let ch = |shift: u32| {
        let av = ((a >> shift) & 0xff) as f64;
        let bv = ((b >> shift) & 0xff) as f64;
        ((av * (1.0 - t) + bv * t).round() as u32) & 0xff
    };
    (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Light-grey fallback for agents whose brand color is too light.
const COLOR_AGENT_FALLBACK: u32 = 0xb8b8c0;

/// Pick a legible text color to render on top of `bg`. White over light
/// pastels (e.g. the agent-fallback grey) reads as a smudge; flip to the dark
/// app background instead when the accent is too light.
fn on_accent_text(bg: u32) -> u32 {
    if relative_luminance(bg) > 0.55 {
        COLOR_BG
    } else {
        0xffffff
    }
}

fn rgba(value: u32) -> gpui::Rgba {
    let r = ((value >> 24) & 0xff) as f32 / 255.0;
    let g = ((value >> 16) & 0xff) as f32 / 255.0;
    let b = ((value >> 8) & 0xff) as f32 / 255.0;
    let a = (value & 0xff) as f32 / 255.0;
    gpui::Rgba { r, g, b, a }
}
