use std::process::Command;

use chrono::Local;
use serde::{Deserialize, Serialize};

// ── Plugin output format (mirrors aura-core::plugin::*) ──────────────────────

#[derive(Serialize)]
struct PluginLine {
    label: String,
    value: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    highlight: bool,
}

#[derive(Serialize)]
struct PluginRow {
    cells: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    highlight: bool,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum PluginContent {
    Lines {
        lines: Vec<PluginLine>,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<PluginRow>,
    },
}

#[derive(Serialize)]
struct PluginSection {
    id: String,
    label: String,
    #[serde(skip_serializing_if = "is_true")]
    uses_period: bool,
    #[serde(flatten)]
    content: PluginContent,
}

fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Serialize)]
struct PluginPanel {
    title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sections: Vec<PluginSection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

// ── `rtk gain -a --format json` schema ───────────────────────────────────────

#[derive(Deserialize)]
struct RtkSummary {
    #[serde(default)]
    total_commands: u64,
    #[serde(default)]
    total_input: u64,
    #[serde(default)]
    total_output: u64,
    #[serde(default)]
    total_saved: u64,
    #[serde(default)]
    avg_savings_pct: f64,
    #[serde(default)]
    total_time_ms: u64,
    #[serde(default)]
    avg_time_ms: u64,
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
struct RtkCommand {
    #[serde(default)]
    command: String,
    #[serde(default)]
    count: u64,
    #[serde(default)]
    saved_tokens: u64,
    #[serde(default)]
    savings_pct: f64,
    #[serde(default)]
    total_time_ms: u64,
}

#[derive(Deserialize)]
struct RtkOutput {
    summary: RtkSummary,
    #[serde(default)]
    daily: Vec<RtkDaily>,
    #[serde(default)]
    monthly: Vec<RtkMonthly>,
    #[serde(default)]
    by_command: Vec<RtkCommand>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

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

/// Render a token count compactly: 681400 -> "681.4K", 2_300_000 -> "2.3M".
fn format_short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// 272_510 ms -> "4m32s"; 7_700 ms -> "7.7s"
fn format_duration_ms(ms: u64) -> String {
    if ms >= 60_000 {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m{secs}s")
    } else if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

/// Trim command to <=24 chars with ellipsis (matches `rtk gain` formatting).
fn truncate_command(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max - 3).collect();
        out.push_str("...");
        out
    }
}

const TITLE: &str = "RTK Gains";

fn emit(panel: &PluginPanel) {
    let _ = serde_json::to_writer(std::io::stdout(), panel);
}

fn emit_error(msg: String) {
    emit(&PluginPanel {
        title: TITLE.to_string(),
        sections: Vec::new(),
        error: Some(msg),
    });
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    // Plugin protocol passes `--period all|7d|30d` (currently unused — `rtk gain`
    // is always all-time today, but we accept the arg so the harness is happy).
    let _args: Vec<String> = std::env::args().collect();

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

    let panel = PluginPanel {
        title: TITLE.to_string(),
        sections: vec![overview_section(&parsed), by_command_section(&parsed)],
        error: None,
    };
    emit(&panel);
}

fn overview_section(parsed: &RtkOutput) -> PluginSection {
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

    PluginSection {
        id: "overview".to_string(),
        label: "Overview".to_string(),
        uses_period: true,
        content: PluginContent::Lines {
            lines: vec![
                PluginLine {
                    label: "Total commands".to_string(),
                    value: format_thousands(parsed.summary.total_commands),
                    highlight: false,
                },
                PluginLine {
                    label: "Input tokens".to_string(),
                    value: format_short(parsed.summary.total_input),
                    highlight: false,
                },
                PluginLine {
                    label: "Output tokens".to_string(),
                    value: format_short(parsed.summary.total_output),
                    highlight: false,
                },
                PluginLine {
                    label: "Tokens saved".to_string(),
                    value: format!(
                        "{} ({:.1}%)",
                        format_short(parsed.summary.total_saved),
                        parsed.summary.avg_savings_pct
                    ),
                    highlight: true,
                },
                PluginLine {
                    label: "Total exec time".to_string(),
                    value: format!(
                        "{} (avg {})",
                        format_duration_ms(parsed.summary.total_time_ms),
                        format_duration_ms(parsed.summary.avg_time_ms)
                    ),
                    highlight: false,
                },
                PluginLine {
                    label: "Saved today".to_string(),
                    value: format_thousands(saved_today),
                    highlight: false,
                },
                PluginLine {
                    label: "Saved this month".to_string(),
                    value: format_thousands(saved_month),
                    highlight: false,
                },
            ],
        },
    }
}

fn by_command_section(parsed: &RtkOutput) -> PluginSection {
    let rows: Vec<PluginRow> = parsed
        .by_command
        .iter()
        .take(10)
        .enumerate()
        .map(|(i, c)| PluginRow {
            cells: vec![
                format!("{}.", i + 1),
                truncate_command(&c.command, 24),
                format_thousands(c.count),
                format_short(c.saved_tokens),
                format!("{:.1}%", c.savings_pct),
                format_duration_ms(c.total_time_ms),
            ],
            highlight: i == 0,
        })
        .collect();

    PluginSection {
        id: "by-command".to_string(),
        label: "By Command".to_string(),
        uses_period: true,
        content: PluginContent::Table {
            headers: vec![
                "#".to_string(),
                "Command".to_string(),
                "Count".to_string(),
                "Saved".to_string(),
                "Avg %".to_string(),
                "Time".to_string(),
            ],
            rows,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_thousands_basic() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(1_000), "1,000");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn format_short_basic() {
        assert_eq!(format_short(42), "42");
        assert_eq!(format_short(1_500), "1.5K");
        assert_eq!(format_short(681_400), "681.4K");
        assert_eq!(format_short(2_300_000), "2.3M");
    }

    #[test]
    fn format_duration_ms_basic() {
        assert_eq!(format_duration_ms(450), "450ms");
        assert_eq!(format_duration_ms(7_700), "7.7s");
        assert_eq!(format_duration_ms(272_510), "4m32s");
    }

    #[test]
    fn truncate_command_basic() {
        assert_eq!(truncate_command("rtk ls", 24), "rtk ls");
        assert_eq!(
            truncate_command("rtk cargo test --workspace --release", 24),
            "rtk cargo test --work..."
        );
    }
}
