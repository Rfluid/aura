//! Reference Aura plugin. Demonstrates the JSON panel contract end-to-end.
//!
//! The host invokes `aura-plugin-hello --period <all|7d|30d>`, reads
//! stdout, and renders the JSON we print. We honour the period so the
//! user sees the pill row update across All / 7d / 30d, but the data is
//! static — extend `data_for_period` with your own metrics source.
//!
//! Wire schema (subset of `aura_core::plugin::PluginPanel`):
//!
//! ```jsonc
//! {
//!   "title": "Hello Plugin",
//!   "sections": [
//!     {
//!       "id": "overview",
//!       "label": "Overview",
//!       "type": "lines",
//!       "lines": [
//!         {"label": "Status", "value": "Running", "highlight": true},
//!         {"label": "Uptime", "value": "0d 0h 5m"}
//!       ]
//!     },
//!     {
//!       "id": "history",
//!       "label": "Recent",
//!       "type": "table",
//!       "headers": ["When", "Event"],
//!       "rows": [{"cells": ["12:34", "Started"]}]
//!     }
//!   ]
//! }
//! ```
//!
//! Surface errors with `"error": "message"` (and omit `sections`) — the
//! host will render it in place of the panel content.

use serde::Serialize;

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

#[derive(Serialize)]
struct PluginPanel {
    title: String,
    sections: Vec<PluginSection>,
}

fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Debug, Clone, Copy)]
enum Period {
    All,
    Last7,
    Last30,
}

impl Period {
    fn from_arg(s: &str) -> Self {
        match s {
            "7d" => Self::Last7,
            "30d" => Self::Last30,
            _ => Self::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "all time",
            Self::Last7 => "last 7 days",
            Self::Last30 => "last 30 days",
        }
    }

    fn events(self) -> u32 {
        match self {
            Self::All => 42,
            Self::Last7 => 7,
            Self::Last30 => 30,
        }
    }
}

fn parse_period() -> Period {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--period" {
            if let Some(v) = args.next() {
                return Period::from_arg(&v);
            }
        }
    }
    Period::All
}

fn main() {
    let period = parse_period();

    let panel = PluginPanel {
        title: "Hello Plugin".to_string(),
        sections: vec![
            PluginSection {
                id: "overview".to_string(),
                label: "Overview".to_string(),
                uses_period: true,
                content: PluginContent::Lines {
                    lines: vec![
                        PluginLine {
                            label: "Period".to_string(),
                            value: period.label().to_string(),
                            highlight: false,
                        },
                        PluginLine {
                            label: "Events".to_string(),
                            value: period.events().to_string(),
                            highlight: true,
                        },
                        PluginLine {
                            label: "Status".to_string(),
                            value: "Running".to_string(),
                            highlight: false,
                        },
                    ],
                },
            },
            PluginSection {
                id: "about".to_string(),
                label: "About".to_string(),
                // Static info — pill row is hidden here so users aren't
                // misled into thinking the contents change with period.
                uses_period: false,
                content: PluginContent::Table {
                    headers: vec!["Field".to_string(), "Value".to_string()],
                    rows: vec![
                        PluginRow {
                            cells: vec!["Plugin".to_string(), "aura-plugin-hello".to_string()],
                        },
                        PluginRow {
                            cells: vec![
                                "Version".to_string(),
                                env!("CARGO_PKG_VERSION").to_string(),
                            ],
                        },
                        PluginRow {
                            cells: vec!["Docs".to_string(), "docs/plugin-authoring.md".to_string()],
                        },
                    ],
                },
            },
        ],
    };

    match serde_json::to_string(&panel) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            // Last-resort error envelope. The host shows this string in
            // place of the panel body — keep it short and actionable.
            println!("{{\"title\":\"Hello Plugin\",\"error\":\"serialize failed: {e}\"}}");
            std::process::exit(1);
        }
    }
}
