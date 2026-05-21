use aura_core::{config::AppConfig, state::AppState};

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

    Ok(())
}
