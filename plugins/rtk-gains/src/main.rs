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
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<f64>,
}

#[derive(Serialize)]
struct PluginRow {
    cells: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    highlight: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<f64>,
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

    let mut parsed: RtkOutput = match serde_json::from_slice(&output.stdout) {
        Ok(p) => p,
        Err(e) => {
            emit_error(format!("could not parse `rtk gain` output: {e}"));
            return;
        }
    };

    // `rtk gain --format json` does not expose the "By Command" breakdown
    // (only summary/daily/weekly/monthly). Recover it by parsing the text
    // output, which the CLI already renders with the same top-10 rollup.
    if parsed.by_command.is_empty() {
        parsed.by_command = fetch_by_command_text();
    }

    let panel = PluginPanel {
        title: TITLE.to_string(),
        sections: vec![overview_section(&parsed), by_command_section(&parsed)],
        error: None,
    };
    emit(&panel);
}

/// Run `rtk gain` in text mode and parse the "By Command" table. Returns
/// an empty Vec on any failure — callers should treat this as best-effort.
fn fetch_by_command_text() -> Vec<RtkCommand> {
    let output = match Command::new("rtk")
        .arg("gain")
        .env("NO_COLOR", "1")
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    parse_by_command(&text)
}

fn parse_by_command(text: &str) -> Vec<RtkCommand> {
    let mut rows = Vec::new();
    let mut in_section = false;
    let mut separators_seen = 0u8;
    for line in text.lines() {
        if !in_section {
            if line.trim_start().starts_with("By Command") {
                in_section = true;
            }
            continue;
        }
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with('─') {
            separators_seen += 1;
            if separators_seen >= 2 && !rows.is_empty() {
                break;
            }
            continue;
        }
        // Column header line.
        if trimmed.contains("Command") && trimmed.contains("Count") {
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some(row) = parse_command_row(trimmed) {
            rows.push(row);
        }
    }
    rows
}

fn parse_command_row(line: &str) -> Option<RtkCommand> {
    // Strip the trailing impact bar (a run of `█` / `░` / whitespace).
    let bar_start = line
        .char_indices()
        .find(|(_, c)| *c == '█' || *c == '░')
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let core = line[..bar_start].trim_end();

    let parts = split_on_runs_of_spaces(core);
    if parts.len() < 6 {
        return None;
    }
    // parts: ["1.", "rtk cargo test --work...", "66", "126.9K", "87.4%", "18.7s"]
    let rank_part = parts[0].trim().trim_end_matches('.');
    rank_part.parse::<u64>().ok()?;

    let command = parts[1].to_string();
    let count: u64 = parts[2].replace(',', "").parse().ok()?;
    let saved_tokens = parse_short_value(parts[3])?;
    let savings_pct: f64 = parts[4].trim_end_matches('%').parse().ok()?;
    let total_time_ms = parse_duration_value(parts[5]).unwrap_or(0);

    Some(RtkCommand {
        command,
        count,
        saved_tokens,
        savings_pct,
        total_time_ms,
    })
}

/// Split on runs of 2+ ASCII spaces, trim each piece, drop empties.
fn split_on_runs_of_spaces(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' {
            let run_start = i;
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            if i - run_start >= 2 {
                let piece = s[start..run_start].trim();
                if !piece.is_empty() {
                    out.push(piece);
                }
                start = i;
                continue;
            }
        } else {
            i += 1;
        }
    }
    let tail = s[start..].trim();
    if !tail.is_empty() {
        out.push(tail);
    }
    out
}

/// "126.9K" → 126_900, "2.3M" → 2_300_000, "42" → 42.
fn parse_short_value(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = if let Some(stripped) = s.strip_suffix('K') {
        (stripped, 1_000.0)
    } else if let Some(stripped) = s.strip_suffix('M') {
        (stripped, 1_000_000.0)
    } else if let Some(stripped) = s.strip_suffix('G') {
        (stripped, 1_000_000_000.0)
    } else {
        (s, 1.0)
    };
    let n: f64 = num.parse().ok()?;
    Some((n * mult) as u64)
}

/// "45ms" → 45, "18.7s" → 18_700, "1m0s" → 60_000, "276m32s" → 16_592_000.
fn parse_duration_value(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(stripped) = s.strip_suffix("ms") {
        return stripped.parse::<f64>().ok().map(|n| n as u64);
    }
    if let Some(m_idx) = s.find('m') {
        let minutes_part = &s[..m_idx];
        let rest = &s[m_idx + 1..];
        if let Some(secs_str) = rest.strip_suffix('s') {
            let mins: u64 = minutes_part.parse().ok()?;
            let secs: f64 = secs_str.parse().ok()?;
            return Some(mins * 60_000 + (secs * 1000.0) as u64);
        }
    }
    if let Some(stripped) = s.strip_suffix('s') {
        return stripped.parse::<f64>().ok().map(|n| (n * 1000.0) as u64);
    }
    None
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
                    progress: None,
                },
                PluginLine {
                    label: "Input tokens".to_string(),
                    value: format_short(parsed.summary.total_input),
                    highlight: false,
                    progress: None,
                },
                PluginLine {
                    label: "Output tokens".to_string(),
                    value: format_short(parsed.summary.total_output),
                    highlight: false,
                    progress: None,
                },
                PluginLine {
                    label: "Tokens saved".to_string(),
                    value: format!(
                        "{} ({:.1}%)",
                        format_short(parsed.summary.total_saved),
                        parsed.summary.avg_savings_pct
                    ),
                    highlight: true,
                    progress: Some((parsed.summary.avg_savings_pct / 100.0).clamp(0.0, 1.0)),
                },
                PluginLine {
                    label: "Total exec time".to_string(),
                    value: format!(
                        "{} (avg {})",
                        format_duration_ms(parsed.summary.total_time_ms),
                        format_duration_ms(parsed.summary.avg_time_ms)
                    ),
                    highlight: false,
                    progress: None,
                },
                PluginLine {
                    label: "Saved today".to_string(),
                    value: format_thousands(saved_today),
                    highlight: false,
                    progress: None,
                },
                PluginLine {
                    label: "Saved this month".to_string(),
                    value: format_thousands(saved_month),
                    highlight: false,
                    progress: None,
                },
            ],
        },
    }
}

fn by_command_section(parsed: &RtkOutput) -> PluginSection {
    // Normalize the Impact bar against the top entry (the command with the
    // most saved tokens). Matches the `rtk gain` CLI rendering.
    let max_saved = parsed
        .by_command
        .iter()
        .take(10)
        .map(|c| c.saved_tokens)
        .max()
        .unwrap_or(0);

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
            progress: if max_saved > 0 {
                Some((c.saved_tokens as f64 / max_saved as f64).clamp(0.0, 1.0))
            } else {
                None
            },
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

    #[test]
    fn parse_short_value_units() {
        assert_eq!(parse_short_value("42"), Some(42));
        assert_eq!(parse_short_value("1.5K"), Some(1_500));
        assert_eq!(parse_short_value("126.9K"), Some(126_900));
        assert_eq!(parse_short_value("2.3M"), Some(2_300_000));
    }

    #[test]
    fn parse_duration_value_units() {
        assert_eq!(parse_duration_value("0ms"), Some(0));
        assert_eq!(parse_duration_value("45ms"), Some(45));
        assert_eq!(parse_duration_value("18.7s"), Some(18_700));
        assert_eq!(parse_duration_value("1m0s"), Some(60_000));
        assert_eq!(parse_duration_value("276m32s"), Some(16_592_000));
    }

    #[test]
    fn parses_by_command_block() {
        let sample = "\
RTK Token Savings (Global Scope)
════════════════════════════════════════════════════════════

Total commands:    2249
Tokens saved:      696.8K (30.5%)

By Command
────────────────────────────────────────────────────────────────────────
  #  Command                   Count   Saved    Avg%    Time  Impact
────────────────────────────────────────────────────────────────────────
 1.  rtk cargo test --work...     66  126.9K   87.4%   18.7s  ██████████
 2.  rtk read                    185   80.4K   19.1%     0ms  ██████░░░░
10.  rtk git branch                1   12.1K   98.8%     9ms  █░░░░░░░░░
────────────────────────────────────────────────────────────────────────
";
        let rows = parse_by_command(sample);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].command, "rtk cargo test --work...");
        assert_eq!(rows[0].count, 66);
        assert_eq!(rows[0].saved_tokens, 126_900);
        assert!((rows[0].savings_pct - 87.4).abs() < 0.01);
        assert_eq!(rows[0].total_time_ms, 18_700);
        assert_eq!(rows[1].command, "rtk read");
        assert_eq!(rows[1].total_time_ms, 0);
        assert_eq!(rows[2].command, "rtk git branch");
        assert_eq!(rows[2].saved_tokens, 12_100);
    }
}
