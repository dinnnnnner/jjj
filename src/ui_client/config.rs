use serde::Deserialize;
use std::fs;
use std::path::Path;

const FEED_ADDR: &str = "127.0.0.1:19011";
const DEFAULT_PG_DSN: &str = "host=127.0.0.1 port=5432 user=postgres password=123456 dbname=demo2";

#[derive(Debug, Clone, Deserialize)]
struct CollectorConfig {
    ui_feed_addr: Option<String>,
    control_addr: Option<String>,
    pg_dsn: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct UiConfig {
    embed_collector: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    collector: Option<CollectorConfig>,
    ui: Option<UiConfig>,
}

fn parse_config(text: &str) -> Result<ConfigFile, toml::de::Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    toml::from_str(text)
}

pub fn load_config_error() -> Option<String> {
    let path = Path::new("config.toml");
    let display_path = std::env::current_dir()
        .map(|dir| dir.join(path))
        .unwrap_or_else(|_| path.to_path_buf());
    match fs::read_to_string(path) {
        Ok(text) => parse_config(&text)
            .err()
            .map(|err| format!("配置文件解析失败（{}）：{err}", display_path.display())),
        Err(err) => Some(format!(
            "配置文件读取失败（{}）：{err}",
            display_path.display()
        )),
    }
}

pub fn load_embed_collector() -> bool {
    let Ok(text) = fs::read_to_string("config.toml") else {
        return false;
    };

    parse_config(&text)
        .ok()
        .and_then(|file| file.ui)
        .and_then(|cfg| cfg.embed_collector)
        .unwrap_or(false)
}

pub fn load_feed_addr() -> String {
    if let Ok(addr) = std::env::var("DEMO2_UI_FEED_ADDR") {
        if !addr.trim().is_empty() {
            return addr;
        }
    }

    let Ok(text) = fs::read_to_string("config.toml") else {
        return FEED_ADDR.to_string();
    };

    match parse_config(&text) {
        Ok(file) => file
            .collector
            .and_then(|cfg| cfg.ui_feed_addr)
            .filter(|addr| !addr.trim().is_empty())
            .unwrap_or_else(|| FEED_ADDR.to_string()),
        Err(_) => FEED_ADDR.to_string(),
    }
}

pub fn load_control_addr() -> String {
    if let Ok(addr) = std::env::var("DEMO2_COLLECTOR_CONTROL_ADDR") {
        if !addr.trim().is_empty() {
            return addr;
        }
    }

    let Ok(text) = fs::read_to_string("config.toml") else {
        return "127.0.0.1:19013".to_string();
    };

    match parse_config(&text) {
        Ok(file) => file
            .collector
            .and_then(|cfg| cfg.control_addr)
            .filter(|addr| !addr.trim().is_empty())
            .unwrap_or_else(|| "127.0.0.1:19013".to_string()),
        Err(_) => "127.0.0.1:19013".to_string(),
    }
}

pub fn load_pg_dsn() -> String {
    if let Ok(dsn) = std::env::var("DEMO2_PG_DSN") {
        if !dsn.trim().is_empty() {
            return dsn;
        }
    }

    let Ok(text) = fs::read_to_string("config.toml") else {
        return DEFAULT_PG_DSN.to_string();
    };

    match parse_config(&text) {
        Ok(file) => file
            .collector
            .and_then(|cfg| cfg.pg_dsn)
            .filter(|dsn| !dsn.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_PG_DSN.to_string()),
        Err(_) => DEFAULT_PG_DSN.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_config;

    fn embed_collector_from(text: &str) -> bool {
        parse_config(text)
            .ok()
            .and_then(|file| file.ui)
            .and_then(|cfg| cfg.embed_collector)
            .unwrap_or(false)
    }

    #[test]
    fn reads_embed_collector_from_ui_section() {
        assert!(embed_collector_from("[ui]\nembed_collector = true\n"));
        assert!(!embed_collector_from("[ui]\nembed_collector = false\n"));
    }

    #[test]
    fn reads_embed_collector_from_utf8_bom_config() {
        assert!(embed_collector_from(
            "\u{feff}[ui]\nembed_collector = true\n"
        ));
    }

    #[test]
    fn embed_collector_defaults_to_false() {
        assert!(!embed_collector_from(
            "[collector]\nui_feed_addr = '127.0.0.1:19011'\n"
        ));
        assert!(!embed_collector_from("not valid toml"));
    }

    #[test]
    fn invalid_config_returns_a_parse_error() {
        assert!(parse_config("not valid toml").is_err());
    }
}
