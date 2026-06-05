//! User-facing UI copy for the modal, swappable at runtime via
//! `display.goblin_mode`. See `docs/goblin-mode.md` for the design rationale.
//!
//! Adding a new user-facing string ⇒ new `Lexicon` field, in **both**
//! personas, in the same PR.

/// All user-facing strings the modal renders. Two const variants live at
/// module scope (`POLITE` and `GOBLIN`); the active one is selected per
/// render via [`pick`].
///
/// A handful of fields are `fn` rather than `&'static str` so persona-
/// specific word order or extra glyphs around the data (subscription tier,
/// reset time, …) stays inside the lexicon instead of leaking into view
/// code.
pub struct Lexicon {
    // ── Tabs ────────────────────────────────────────────────────────────────
    pub tab_quota: &'static str,
    pub tab_summary: &'static str,
    pub tab_models: &'static str,
    pub tab_plugins: &'static str,
    pub tab_forecast: &'static str,

    // ── Forecast status badges & subtext ───────────────────────────────────
    pub forecast_ok: &'static str,
    pub forecast_watch: &'static str,
    pub forecast_overshoot: &'static str,
    pub forecast_insufficient: &'static str,
    pub forecast_projected_at_reset: &'static str,
    pub forecast_will_hit_100_fmt: fn(time: &str) -> String,
    pub forecast_warming_up: &'static str,

    // ── Session budget gauge (F2 pacing) ───────────────────────────────────
    pub pacing_title: &'static str,
    pub pacing_ok: &'static str,
    pub pacing_watch: &'static str,
    pub pacing_over: &'static str,
    pub pacing_insufficient: &'static str,
    /// "Spend up to ~38% of this 5h session" — takes the recommended ceiling.
    pub pacing_spend_up_to_fmt: fn(pct: f64) -> String,

    // ── Empty / loading ─────────────────────────────────────────────────────
    pub loading: &'static str,
    pub no_quota_data: &'static str,
    pub no_plugins_configured: &'static str,
    pub no_plugin_selected: &'static str,

    // ── Quota row chrome ────────────────────────────────────────────────────
    pub subscription_fmt: fn(sub: &str) -> String,
    pub resets_fmt: fn(when: &str) -> String,

    // ── More menu ───────────────────────────────────────────────────────────
    pub menu_open_config: &'static str,
    pub menu_themes: &'static str,
    /// Modal-row label for "Check updates", with the running version
    /// appended (e.g. "Check updates · v0.1.17"). Goblin gets to roast
    /// the user's current build.
    pub check_updates_fmt: fn(current_version: &str) -> String,

    // ── Update button ───────────────────────────────────────────────────────
    /// Header chip label when a newer release exists. The arrow at the
    /// end is part of the persona — keep or replace as you see fit.
    pub update_available_fmt: fn(latest_version: &str) -> String,

    // ── Period pills ────────────────────────────────────────────────────────
    pub period_all: &'static str,
    pub period_7d: &'static str,
    pub period_30d: &'static str,
}

fn polite_subscription(sub: &str) -> String {
    format!("Subscription: {sub}")
}
fn polite_resets(when: &str) -> String {
    format!("Resets {when}")
}
fn polite_will_hit_100(time: &str) -> String {
    format!("Will hit 100% at {time}")
}
fn polite_pacing_spend_up_to(pct: f64) -> String {
    format!("Spend up to ~{pct:.0}% of this 5h session")
}
fn polite_check_updates(current: &str) -> String {
    format!("Check updates · v{current}")
}
fn polite_update_available(latest: &str) -> String {
    format!("Update available · v{latest} →")
}

fn goblin_subscription(sub: &str) -> String {
    format!("Paying for: {sub}")
}
fn goblin_resets(when: &str) -> String {
    format!("Wipes {when}")
}
fn goblin_will_hit_100(time: &str) -> String {
    format!("Goose is cooked at {time}")
}
fn goblin_pacing_spend_up_to(pct: f64) -> String {
    format!("Blow ~{pct:.0}% of this 5h, tops")
}
fn goblin_check_updates(current: &str) -> String {
    format!("Running ancient · v{current}")
}
fn goblin_update_available(latest: &str) -> String {
    format!("New build dropped · v{latest} →")
}

/// Default voice. Verbatim copy of what the modal renders today — flipping
/// `goblin_mode` off must produce an identical UI.
pub const POLITE: Lexicon = Lexicon {
    tab_quota: "Quota",
    tab_summary: "Summary",
    tab_models: "Models",
    tab_plugins: "Plugins",
    tab_forecast: "Forecast",

    forecast_ok: "On track",
    forecast_watch: "Watch",
    forecast_overshoot: "Overshoot",
    forecast_insufficient: "—",
    forecast_projected_at_reset: "Projected at reset",
    forecast_will_hit_100_fmt: polite_will_hit_100,
    forecast_warming_up: "warming up — check back in a few minutes",

    pacing_title: "Session budget",
    pacing_ok: "On track",
    pacing_watch: "Watch",
    pacing_over: "Over",
    pacing_insufficient: "—",
    pacing_spend_up_to_fmt: polite_pacing_spend_up_to,

    loading: "Loading…",
    no_quota_data: "No quota data available.",
    no_plugins_configured: "No plugins configured",
    no_plugin_selected: "No plugin selected",

    subscription_fmt: polite_subscription,
    resets_fmt: polite_resets,

    menu_open_config: "Open config file",
    menu_themes: "Themes",
    check_updates_fmt: polite_check_updates,

    update_available_fmt: polite_update_available,

    period_all: "All time",
    period_7d: "Last 7 days",
    period_30d: "Last 30 days",
};

/// Alt persona — gremlin energy, mildly hostile, never aimed at the user.
/// See `docs/goblin-mode.md` for the voice rules.
pub const GOBLIN: Lexicon = Lexicon {
    tab_quota: "Damage",
    tab_summary: "The Bill",
    tab_models: "Slop Vendors",
    tab_plugins: "Hangers-on",
    tab_forecast: "Doom",

    forecast_ok: "Fine, whatever",
    forecast_watch: "Bruh",
    forecast_overshoot: "Cooked",
    forecast_insufficient: "Idk yet",
    forecast_projected_at_reset: "What you'll burn",
    forecast_will_hit_100_fmt: goblin_will_hit_100,
    forecast_warming_up: "give it a minute, jeez",

    pacing_title: "Burn budget",
    pacing_ok: "Fine",
    pacing_watch: "Bruh",
    pacing_over: "Over it",
    pacing_insufficient: "Idk yet",
    pacing_spend_up_to_fmt: goblin_pacing_spend_up_to,

    loading: "hold on damn",
    no_quota_data: "Nothing. Empty. Dry.",
    no_plugins_configured: "No hangers-on",
    no_plugin_selected: "Pick one, coward",

    subscription_fmt: goblin_subscription,
    resets_fmt: goblin_resets,

    menu_open_config: "Crack open the config",
    menu_themes: "Paint job",
    check_updates_fmt: goblin_check_updates,

    update_available_fmt: goblin_update_available,

    period_all: "the whole damn time",
    period_7d: "last week",
    period_30d: "this month-ish",
};

/// Resolve the active lexicon from the `goblin_mode` config flag.
pub fn pick(goblin_mode: bool) -> &'static Lexicon {
    if goblin_mode {
        &GOBLIN
    } else {
        &POLITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (field name, polite value, goblin value) for every `&'static str` entry
    /// in the lexicon. Used by the length-budget / profanity tests so adding a
    /// new field forces a same-PR update here.
    fn pairs() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("tab_quota", POLITE.tab_quota, GOBLIN.tab_quota),
            ("tab_summary", POLITE.tab_summary, GOBLIN.tab_summary),
            ("tab_models", POLITE.tab_models, GOBLIN.tab_models),
            ("tab_plugins", POLITE.tab_plugins, GOBLIN.tab_plugins),
            ("tab_forecast", POLITE.tab_forecast, GOBLIN.tab_forecast),
            ("forecast_ok", POLITE.forecast_ok, GOBLIN.forecast_ok),
            (
                "forecast_watch",
                POLITE.forecast_watch,
                GOBLIN.forecast_watch,
            ),
            (
                "forecast_overshoot",
                POLITE.forecast_overshoot,
                GOBLIN.forecast_overshoot,
            ),
            (
                "forecast_insufficient",
                POLITE.forecast_insufficient,
                GOBLIN.forecast_insufficient,
            ),
            (
                "forecast_projected_at_reset",
                POLITE.forecast_projected_at_reset,
                GOBLIN.forecast_projected_at_reset,
            ),
            (
                "forecast_warming_up",
                POLITE.forecast_warming_up,
                GOBLIN.forecast_warming_up,
            ),
            ("pacing_title", POLITE.pacing_title, GOBLIN.pacing_title),
            ("pacing_ok", POLITE.pacing_ok, GOBLIN.pacing_ok),
            ("pacing_watch", POLITE.pacing_watch, GOBLIN.pacing_watch),
            ("pacing_over", POLITE.pacing_over, GOBLIN.pacing_over),
            (
                "pacing_insufficient",
                POLITE.pacing_insufficient,
                GOBLIN.pacing_insufficient,
            ),
            ("loading", POLITE.loading, GOBLIN.loading),
            ("no_quota_data", POLITE.no_quota_data, GOBLIN.no_quota_data),
            (
                "no_plugins_configured",
                POLITE.no_plugins_configured,
                GOBLIN.no_plugins_configured,
            ),
            (
                "no_plugin_selected",
                POLITE.no_plugin_selected,
                GOBLIN.no_plugin_selected,
            ),
            (
                "menu_open_config",
                POLITE.menu_open_config,
                GOBLIN.menu_open_config,
            ),
            ("menu_themes", POLITE.menu_themes, GOBLIN.menu_themes),
            ("period_all", POLITE.period_all, GOBLIN.period_all),
            ("period_7d", POLITE.period_7d, GOBLIN.period_7d),
            ("period_30d", POLITE.period_30d, GOBLIN.period_30d),
        ]
    }

    /// Goblin entries must not exceed their polite counterpart by more than
    /// ~12 chars. Tab labels and badges share a flex row that wraps awkwardly
    /// past that — see `docs/goblin-mode.md` §Safety rails.
    #[test]
    fn goblin_length_budget() {
        const BUDGET: usize = 12;
        for (label, polite, goblin) in pairs() {
            let p = polite.chars().count();
            let g = goblin.chars().count();
            assert!(
                g <= p + BUDGET,
                "{label}: goblin ({g} chars) exceeds polite ({p} chars) + {BUDGET}",
            );
        }
    }

    /// Persona is allowed to be coarse; targeted slurs and protected-class
    /// shots are not. Reviewers extend the deny-list as new patterns surface.
    #[test]
    fn goblin_profanity_boundary() {
        // Intentionally minimal — extend in review when a real pattern lands.
        // Kept here so that any future GOBLIN edit that accidentally includes
        // one of these substrings fails CI rather than shipping.
        const BANNED: &[&str] = &[
            // No live entries yet. Adding examples here would put slurs in
            // the source tree; reviewers add specific tokens when they
            // catch one in a PR.
        ];
        for (label, _, goblin) in pairs() {
            let g = goblin.to_lowercase();
            for needle in BANNED {
                assert!(
                    !g.contains(needle),
                    "{label}: goblin string contains banned token {needle:?}",
                );
            }
        }
    }

    #[test]
    fn pick_matches_flag() {
        // `const` items don't have a stable address (each reference may create
        // a fresh copy in the binary), so compare a tagging field instead of
        // pointers.
        assert_eq!(pick(false).tab_quota, POLITE.tab_quota);
        assert_eq!(pick(true).tab_quota, GOBLIN.tab_quota);
        assert_ne!(POLITE.tab_quota, GOBLIN.tab_quota);
    }

    #[test]
    fn polite_formatters_match_legacy_copy() {
        assert_eq!((POLITE.subscription_fmt)("pro"), "Subscription: pro");
        assert_eq!((POLITE.resets_fmt)("in 3h"), "Resets in 3h");
        assert_eq!(
            (POLITE.forecast_will_hit_100_fmt)("5pm"),
            "Will hit 100% at 5pm",
        );
        assert_eq!(
            (POLITE.check_updates_fmt)("0.1.17"),
            "Check updates · v0.1.17",
        );
        assert_eq!(
            (POLITE.update_available_fmt)("0.1.18"),
            "Update available · v0.1.18 →",
        );
    }

    #[test]
    fn goblin_formatters_use_persona_words() {
        assert_eq!((GOBLIN.subscription_fmt)("pro"), "Paying for: pro");
        assert_eq!((GOBLIN.resets_fmt)("in 3h"), "Wipes in 3h");
        assert_eq!(
            (GOBLIN.forecast_will_hit_100_fmt)("5pm"),
            "Goose is cooked at 5pm",
        );
        assert_eq!(
            (GOBLIN.check_updates_fmt)("0.1.17"),
            "Running ancient · v0.1.17",
        );
        assert_eq!(
            (GOBLIN.update_available_fmt)("0.1.18"),
            "New build dropped · v0.1.18 →",
        );
    }
}
