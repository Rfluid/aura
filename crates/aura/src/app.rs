use aura_core::{
    config::AppConfig,
    plugin::{PluginPanel, PluginRunner},
    reader::{make_reader, Period, UsageSnapshot},
    state::AppState,
};
use gpui::{div, prelude::*, px, rgb, AnyElement, ClickEvent, Context, SharedString, Window};

use crate::format::{duration, hour_of_day, thousands};

// ── Theme tokens (Zed-ish dark) ───────────────────────────────────────────────
const COLOR_BG: u32 = 0x0e0e10;
const COLOR_SURFACE: u32 = 0x1a1a1f;
const COLOR_SURFACE_HI: u32 = 0x252530;
const COLOR_BORDER: u32 = 0x2d2d36;
const COLOR_TEXT: u32 = 0xe6e6ee;
const COLOR_TEXT_DIM: u32 = 0x8a8a9a;
const COLOR_ACCENT: u32 = 0x8b5cf6;
const COLOR_ACCENT_DIM: u32 = 0x4c1d95;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Models,
}

pub struct AuraView {
    config: AppConfig,
    state: AppState,
    active_profile: String,
    active_period: Period,
    active_tab: Tab,
    snapshot: Option<UsageSnapshot>,
    plugin_panels: Vec<PluginPanel>,
    error: Option<String>,
}

impl AuraView {
    pub fn new(config: AppConfig, state: AppState, _cx: &mut Context<Self>) -> Self {
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
            state,
            active_profile,
            active_period,
            active_tab: Tab::Overview,
            snapshot: None,
            plugin_panels: Vec::new(),
            error: None,
        };
        view.refresh();
        view
    }

    fn refresh(&mut self) {
        self.error = None;
        let Some(agent) = self
            .config
            .agents
            .iter()
            .find(|a| a.name == self.active_profile)
        else {
            self.snapshot = None;
            self.error = Some(format!(
                "Profile `{}` not found in config",
                self.active_profile
            ));
            return;
        };

        let reader = make_reader(agent);
        match reader.snapshot(self.active_period) {
            Ok(snap) => self.snapshot = Some(snap),
            Err(e) => {
                self.snapshot = None;
                self.error = Some(format!("Snapshot failed: {e}"));
            }
        }

        self.plugin_panels = self.config.plugins.iter().map(PluginRunner::run).collect();
    }

    fn set_profile(&mut self, name: String, cx: &mut Context<Self>) {
        if self.active_profile != name {
            self.active_profile = name.clone();
            self.state.active_profile = Some(name);
            let _ = self.state.save();
            self.refresh();
            cx.notify();
        }
    }

    fn set_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if self.active_period != period {
            self.active_period = period;
            self.refresh();
            cx.notify();
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.active_tab != tab {
            self.active_tab = tab;
            cx.notify();
        }
    }
}

// ── Render ────────────────────────────────────────────────────────────────────

impl Render for AuraView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(COLOR_BG))
            .text_color(rgb(COLOR_TEXT))
            .font_family("monospace")
            .text_sm()
            .child(self.render_header(cx))
            .child(self.render_period_row(cx))
            .child(self.render_tab_row(cx))
            .child(self.render_body(cx))
            .child(self.render_plugins(cx))
    }
}

// ── Sub-renderers ─────────────────────────────────────────────────────────────

impl AuraView {
    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER))
            .bg(rgb(COLOR_SURFACE));

        // Profile pills
        let mut picker = div().flex().flex_row().gap_2();
        for agent in &self.config.agents {
            let name = agent.name.clone();
            let active = self.active_profile == agent.name;
            let pill_name = name.clone();
            picker = picker.child(
                div()
                    .id(SharedString::from(format!("profile-{}", agent.name)))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .text_xs()
                    .when(active, |d| {
                        d.bg(rgb(COLOR_ACCENT_DIM)).text_color(rgb(COLOR_TEXT))
                    })
                    .when(!active, |d| {
                        d.bg(rgb(COLOR_SURFACE_HI)).text_color(rgb(COLOR_TEXT_DIM))
                    })
                    .child(SharedString::from(name))
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.set_profile(pill_name.clone(), cx);
                    })),
            );
        }
        row = row.child(picker);
        row = row.child(
            div()
                .id("title-refresh")
                .text_color(rgb(COLOR_ACCENT))
                .child("Aura ⟳")
                .on_click(cx.listener(|view, _: &ClickEvent, _, cx| {
                    view.refresh();
                    cx.notify();
                })),
        );
        row.into_any_element()
    }

    fn render_period_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let periods = [
            ("All time", Period::AllTime, "period-all"),
            ("Last 7 days", Period::Last7Days, "period-7"),
            ("Last 30 days", Period::Last30Days, "period-30"),
        ];

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
                    .when(active, |d| {
                        d.bg(rgb(COLOR_ACCENT)).text_color(rgb(0xffffff))
                    })
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
        let tabs = [
            ("Overview", Tab::Overview, "tab-overview"),
            ("Models", Tab::Models, "tab-models"),
        ];

        let mut row = div()
            .flex()
            .flex_row()
            .gap_4()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(rgb(COLOR_BORDER))
            .bg(rgb(COLOR_SURFACE));

        for (label, tab, id) in tabs {
            let active = self.active_tab == tab;
            row = row.child(
                div()
                    .id(SharedString::from(id))
                    .text_sm()
                    .pb_1()
                    .when(active, |d| {
                        d.text_color(rgb(COLOR_TEXT))
                            .border_b_2()
                            .border_color(rgb(COLOR_ACCENT))
                    })
                    .when(!active, |d| d.text_color(rgb(COLOR_TEXT_DIM)))
                    .child(label)
                    .on_click(cx.listener(move |view, _: &ClickEvent, _, cx| {
                        view.set_tab(tab, cx);
                    })),
            );
        }
        row.into_any_element()
    }

    fn render_body(&self, _cx: &mut Context<Self>) -> AnyElement {
        if let Some(err) = &self.error {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .p_6()
                .child(div().text_color(rgb(0xff6b6b)).child(err.clone()))
                .into_any_element();
        }

        let Some(snap) = self.snapshot.as_ref() else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .justify_center()
                .child(div().text_color(rgb(COLOR_TEXT_DIM)).child("Loading…"))
                .into_any_element();
        };

        match self.active_tab {
            Tab::Overview => render_overview(snap),
            Tab::Models => render_models(snap),
        }
    }

    fn render_plugins(&self, _cx: &mut Context<Self>) -> AnyElement {
        if self.plugin_panels.is_empty() {
            return div().into_any_element();
        }
        let mut col = div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(rgb(COLOR_BORDER))
            .bg(rgb(COLOR_SURFACE));

        for panel in &self.plugin_panels {
            col = col.child(render_plugin_panel(panel));
        }
        col.into_any_element()
    }
}

// ── Overview ──────────────────────────────────────────────────────────────────

fn render_overview(snap: &UsageSnapshot) -> AnyElement {
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

    let mut col = div().flex().flex_col().flex_1().px_4().py_3().gap_2();

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

fn render_models(snap: &UsageSnapshot) -> AnyElement {
    let mut col = div().flex().flex_col().flex_1().px_4().py_3().gap_4();

    // Tokens per Day chart
    col = col.child(render_daily_chart(snap));

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
        models_col = models_col.child(render_model_row(&m.model, tokens, pct));
    }
    col = col.child(models_col);
    col.into_any_element()
}

fn render_model_row(model: &str, tokens: u64, pct: f64) -> impl IntoElement {
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
                        .bg(rgb(COLOR_ACCENT))
                        .rounded_md(),
                ),
        )
}

fn render_daily_chart(snap: &UsageSnapshot) -> impl IntoElement {
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
        bars = bars.child(
            div()
                .flex_1()
                .h(px(height))
                .bg(rgb(COLOR_ACCENT))
                .rounded_sm(),
        );
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

// ── Plugin panel ──────────────────────────────────────────────────────────────

fn render_plugin_panel(panel: &PluginPanel) -> impl IntoElement {
    let mut col = div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_2()
        .bg(rgb(COLOR_BG))
        .rounded_md()
        .border_1()
        .border_color(rgb(COLOR_BORDER))
        .child(
            div()
                .text_xs()
                .text_color(rgb(COLOR_ACCENT))
                .child(SharedString::from(panel.title.clone())),
        );

    if let Some(err) = &panel.error {
        col = col.child(
            div()
                .text_xs()
                .text_color(rgb(0xff6b6b))
                .child(SharedString::from(err.clone())),
        );
    } else {
        for line in &panel.lines {
            col = col.child(
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
                            .when(line.highlight, |d| d.text_color(rgb(COLOR_ACCENT)))
                            .when(!line.highlight, |d| d.text_color(rgb(COLOR_TEXT)))
                            .child(SharedString::from(line.value.clone())),
                    ),
            );
        }
    }
    col
}
