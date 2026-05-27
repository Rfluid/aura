use std::{cell::Cell, path::PathBuf, rc::Rc, time::Duration};

use aura_core::{
    config::{AgentConfig, AgentKind, AppConfig, PluginConfig},
    lexicon::{self, Lexicon},
    plugin::{PluginContent, PluginPanel, PluginRunner, PluginSection},
    quota::{
        forecast, CodexQuota, ForecastSnapshot, ForecastStatus, ForecastWindow, GeminiQuota,
        QuotaApi, QuotaSnapshot, QuotaSource, QuotaWindow,
    },
    reader::{make_reader, Period, UsageSnapshot},
    state::AppState,
    theme::Theme,
};
use chrono::{DateTime, Local, Timelike, Utc};
use gpui::{
    div, prelude::*, px, rgb, size, svg, AnyElement, ClickEvent, Context, Pixels, ScrollHandle,
    SharedString, Window,
};

use crate::format::{duration, hour_of_day, locale_uses_12h, system_locale, thousands};

/// Fixed window width. The window grows vertically to fit content (see
/// `on_children_prepainted` in `render`), so only the height is dynamic.
const WINDOW_WIDTH: f32 = 520.0;

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
}

impl AgentSection {
    fn label(self, lex: &Lexicon) -> &'static str {
        match self {
            Self::Quota => lex.tab_quota,
            Self::Forecast => lex.tab_forecast,
            Self::Summary => lex.tab_summary,
            Self::Models => lex.tab_models,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Self::Quota => "quota",
            Self::Forecast => "forecast",
            Self::Summary => "summary",
            Self::Models => "models",
        }
    }

    /// Whether this section filters data by the active period. Quota
    /// reports rolling 5h / 7d subscription windows fixed by the API, so
    /// the period pills don't apply.
    fn uses_period(self) -> bool {
        match self {
            Self::Quota | Self::Forecast => false,
            Self::Summary | Self::Models => true,
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
    forecast: Option<ForecastSnapshot>,
    /// Indexed by plugin name.
    plugin_panels: Vec<(String, PluginPanel)>,

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
            forecast: None,
            plugin_panels: Vec::new(),
            show_more_modal: false,
            show_settings_panel: false,
            is_loading: false,
            spinner_frame: 0,
            error: None,
            last_window_height: Rc::new(Cell::new(Pixels::ZERO)),
            body_scroll: ScrollHandle::new(),
            needs_uncloak: Rc::new(Cell::new(cfg!(target_os = "windows"))),
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
            self.config = cfg;
        }
        if let Some(theme) = result.theme {
            self.theme = theme;
        }
        if let Some(fallback) = result.fallback_profile {
            self.active_profile = fallback;
        }
        self.snapshot = result.snapshot;
        self.quota = result.quota;
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
    let quota = Some(match agent.kind {
        AgentKind::ClaudeCode => QuotaApi::new(agent_path).snapshot(),
        AgentKind::Codex => CodexQuota::new(agent_path).snapshot(),
        AgentKind::Gemini => GeminiQuota::new(agent_path).snapshot(),
    });

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
        // When window_chrome is on, the OS draws a title bar and the user can
        // drag the edges — so suppress the content-fit auto-resize that would
        // otherwise snap the height back every layout pass.
        let auto_fit = !self.config.display.window_chrome;
        let user_max_height = self.config.display.max_height;
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
                #[cfg(target_os = "windows")]
                let uncloak = needs_uncloak.clone();
                window.on_next_frame(move |window, _cx| {
                    window.resize(new_size);
                    // On Windows: GPUI's resize() uses SWP_NOMOVE, which keeps
                    // the window's top-left origin fixed and moves the bottom.
                    // We want the bottom to stay flush with the taskbar, so we
                    // reposition to `work_bottom - content_h - gap` after the
                    // resize.  Both ops are spawned on the ForegroundExecutor
                    // (FIFO): resize task runs first, then reposition+uncloak,
                    // so DWM sees only the final state at the next vsync.
                    #[cfg(target_os = "windows")]
                    {
                        let scale = window.scale_factor();
                        let cur_x_phys =
                            (f32::from(window.bounds().origin.x) * scale).round() as i32;
                        let target_y_phys = if let Some(display) = _cx.primary_display() {
                            let db = display.bounds();
                            let full_bottom = f32::from(db.origin.y + db.size.height);
                            let work_bottom = crate::work_area::available_bottom(db)
                                .unwrap_or(full_bottom - 120.0);
                            let y = work_bottom - f32::from(new_size.height) - 8.0;
                            (y * scale).round() as i32
                        } else {
                            // No display info — fall back to sync uncloak only.
                            if uncloak.replace(false) {
                                crate::win32_set_cloak(window, false);
                            }
                            return;
                        };
                        let hwnd_val = window.hwnd();
                        let do_uncloak = uncloak.replace(false);
                        _cx.spawn(async move |_cx| {
                            use windows::Win32::Foundation::HWND;
                            use windows::Win32::UI::WindowsAndMessaging::{
                                SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
                            };
                            let hwnd = HWND(hwnd_val as *mut _);
                            unsafe {
                                let _ = SetWindowPos(
                                    hwnd,
                                    None,
                                    cur_x_phys,
                                    target_y_phys,
                                    0,
                                    0,
                                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                                );
                            }
                            if do_uncloak {
                                use windows::Win32::Graphics::Dwm::{
                                    DwmSetWindowAttribute, DWMWA_CLOAK,
                                };
                                let val: i32 = 0;
                                unsafe {
                                    let _ = DwmSetWindowAttribute(
                                        hwnd,
                                        DWMWA_CLOAK,
                                        std::ptr::addr_of!(val).cast(),
                                        std::mem::size_of::<i32>() as u32,
                                    );
                                }
                            }
                        })
                        .detach();
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

        // Right: action buttons (refresh, config, more)
        let actions = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
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
                let sections = [
                    AgentSection::Quota,
                    AgentSection::Forecast,
                    AgentSection::Summary,
                    AgentSection::Models,
                ];
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

    fn render_body(&self, _cx: &mut Context<Self>) -> AnyElement {
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

fn render_forecast(
    theme: &Theme,
    lex: &Lexicon,
    forecast: Option<&ForecastSnapshot>,
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

    col.into_any_element()
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

fn render_daily_chart(theme: &Theme, snap: &UsageSnapshot, accent: u32) -> impl IntoElement {
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
        let items = [
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
                "modal-github",
                "icons/github.svg",
                "View on GitHub",
                ModalAction::Github,
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
                    .child(label)
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.handle_modal_action(action, cx);
                    })),
            );
        }

        backdrop.child(card).into_any_element()
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
