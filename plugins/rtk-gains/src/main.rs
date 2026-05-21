use std::process::Command;

use chrono::Local;
use serde::{Deserialize, Serialize};

// ── Plugin output format (must mirror aura-core::plugin::PluginPanel) ─────────

#[derive(Serialize)]
struct PluginLine {
    label: String,
    value: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    highlight: bool,
}

#[derive(Serialize)]
struct PluginPanel {
    title: String,
    lines: Vec<PluginLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ── `rtk gain -a --format json` schema ────────────────────────────────────────

#[derive(Deserialize)]
struct RtkSummary {
    #[serde(default)]
    total_commands: u64,
    #[serde(default)]
    total_saved: u64,
    #[serde(default)]
    avg_savings_pct: f64,
}

#[derive(Deserialize)]
struct RtkDaily {
    date: String,
    #[serde(default)]
    saved_tokens: u64,
}

#[derive(Deserialize)]
struct RtkMonthly {
    month: String,
    #[serde(default)]
    saved_tokens: u64,
}

#[derive(Deserialize)]
struct RtkOutput {
    summary: RtkSummary,
    #[serde(default)]
    daily: Vec<RtkDaily>,
    #[serde(default)]
    monthly: Vec<RtkMonthly>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn format_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

const TITLE: &str = "RTK Gains";

fn emit(panel: &PluginPanel) {
    let _ = serde_json::to_writer(std::io::stdout(), panel);
}

fn emit_error(msg: String) {
    emit(&PluginPanel {
        title: TITLE.to_string(),
        lines: Vec::new(),
        error: Some(msg),
    });
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let output = match Command::new("rtk")
        .args(["gain", "-a", "--format", "json"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            emit_error(format!("`rtk` not found on PATH: {e}"));
            return;
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            format!("`rtk gain` exited with status {}", output.status)
        } else {
            stderr
        };
        emit_error(msg);
        return;
    }

    let parsed: RtkOutput = match serde_json::from_slice(&output.stdout) {
        Ok(p) => p,
        Err(e) => {
            emit_error(format!("could not parse `rtk gain` output: {e}"));
            return;
        }
    };

    let now = Local::now();
    let today = now.format("%Y-%m-%d").to_string();
    let this_month = now.format("%Y-%m").to_string();

    let saved_today = parsed
        .daily
        .iter()
        .find(|d| d.date == today)
        .map(|d| d.saved_tokens)
        .unwrap_or(0);

    let saved_month = parsed
        .monthly
        .iter()
        .find(|m| m.month == this_month)
        .map(|m| m.saved_tokens)
        .unwrap_or(0);

    let panel = PluginPanel {
        title: TITLE.to_string(),
        lines: vec![
            PluginLine {
                label: "Tokens saved today".to_string(),
                value: format_thousands(saved_today),
                highlight: true,
            },
            PluginLine {
                label: "Tokens saved this month".to_string(),
                value: format_thousands(saved_month),
                highlight: false,
            },
            PluginLine {
                label: "Tokens saved (all time)".to_string(),
                value: format_thousands(parsed.summary.total_saved),
                highlight: false,
            },
            PluginLine {
                label: "Savings rate".to_string(),
                value: format!("{:.1}%", parsed.summary.avg_savings_pct),
                highlight: false,
            },
            PluginLine {
                label: "Commands intercepted".to_string(),
                value: format_thousands(parsed.summary.total_commands),
                highlight: false,
            },
        ],
        error: None,
    };

    emit(&panel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_thousands_basic() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(42), "42");
        assert_eq!(format_thousands(1_000), "1,000");
        assert_eq!(format_thousands(12_345), "12,345");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn parses_real_rtk_output() {
        let sample = r#"{
            "summary": {
                "total_commands": 1928,
                "total_input": 2199470,
                "total_output": 1562732,
                "total_saved": 639673,
                "avg_savings_pct": 29.08
            },
            "daily": [
                {"date":"2026-05-20","saved_tokens":10000},
                {"date":"2026-05-21","saved_tokens":31329}
            ],
            "monthly": [
                {"month":"2026-05","saved_tokens":639673}
            ]
        }"#;
        let parsed: RtkOutput = serde_json::from_str(sample).unwrap();
        assert_eq!(parsed.summary.total_commands, 1928);
        assert_eq!(parsed.daily.len(), 2);
        assert_eq!(parsed.monthly[0].saved_tokens, 639673);
    }
}
