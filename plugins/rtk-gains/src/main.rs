use serde::Serialize;

#[derive(Serialize)]
struct PluginLine {
    label: String,
    value: String,
    #[serde(default)]
    highlight: bool,
}

#[derive(Serialize)]
struct PluginPanel {
    title: String,
    lines: Vec<PluginLine>,
    error: Option<String>,
}

fn main() {
    let panel = PluginPanel {
        title: "RTK Gains".to_string(),
        lines: vec![],
        error: Some("not yet implemented".to_string()),
    };
    println!("{}", serde_json::to_string(&panel).unwrap());
}
