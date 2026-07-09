use serde::Deserialize;
use std::fs;

const FEED_ADDR: &str = "127.0.0.1:19011";
const DEFAULT_PG_DSN: &str = "host=127.0.0.1 port=5432 user=postgres password=123456 dbname=demo2";

#[derive(Debug, Clone, Deserialize)]
struct CollectorConfig {
    ui_feed_addr: Option<String>,
    control_addr: Option<String>,
    pg_dsn: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    collector: Option<CollectorConfig>,
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

    match toml::from_str::<ConfigFile>(&text) {
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

    match toml::from_str::<ConfigFile>(&text) {
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

    match toml::from_str::<ConfigFile>(&text) {
        Ok(file) => file
            .collector
            .and_then(|cfg| cfg.pg_dsn)
            .filter(|dsn| !dsn.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_PG_DSN.to_string()),
        Err(_) => DEFAULT_PG_DSN.to_string(),
    }
}
