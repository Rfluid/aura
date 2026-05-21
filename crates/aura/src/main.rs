use aura_core::{
    config::AppConfig,
    reader::{AgentReader, ClaudeCodeReader, Period},
    state::AppState,
};

fn main() -> anyhow::Result<()> {
    let config_path = AppConfig::default_path();
    let config = AppConfig::load(&config_path)?;

    let state = AppState::load()?;

    let active = state
        .active_profile
        .as_deref()
        .or_else(|| config.agents.first().map(|a| a.name.as_str()))
        .unwrap_or("(no profiles configured)");

    println!("Aura — Agent Usage Reporter & Analyzer");
    println!("Active profile : {active}");
    println!("Config         : {}", config_path.display());
    println!("Agents         : {}", config.agents.len());
    println!("Plugins        : {}", config.plugins.len());

    // ── Smoke-test: read snapshot from the first Claude Code agent ────────────
    if let Some(agent) = config.agents.first() {
        let claude_path = agent.resolved_config_path();
        let reader = ClaudeCodeReader::new(claude_path);
        match reader.snapshot(Period::Last7Days) {
            Ok(snap) => {
                println!("\n── Last 7 Days ───────────────────────────────────");
                println!("Total tokens   : {}", snap.total_tokens);
                println!("Sessions       : {}", snap.total_sessions);
                println!("Active days    : {}", snap.active_days);
                println!(
                    "Favorite model : {}",
                    snap.favorite_model.as_deref().unwrap_or("—")
                );
                println!("Current streak : {} day(s)", snap.streaks.current);
            }
            Err(e) => eprintln!("snapshot error: {e}"),
        }
    }

    Ok(())
}
