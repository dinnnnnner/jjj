use crate::transport::can::{CanChannelConfig, HW_SUBTYPE_TC1016};
use serde::Deserialize;
use std::fs;
use tracing::warn;

const DEFAULT_DB_FILTER_ORDER: usize = 10;
const DEFAULT_DB_FILTER_SAMPLE_RATE_HZ: f32 = 48_000.0;
const DEFAULT_DB_FILTER_CUTOFF_HZ: f32 = 4_000.0;
const DEFAULT_SENT_FILTER_WINDOW: usize = 10;
const DEFAULT_CAN_SIGNAL_TIMEOUT_MS: u64 = 2_000;

fn default_db_filter_enabled() -> bool {
    false
}

fn default_db_filter_order() -> usize {
    DEFAULT_DB_FILTER_ORDER
}

fn default_db_filter_sample_rate_hz() -> f32 {
    DEFAULT_DB_FILTER_SAMPLE_RATE_HZ
}

fn default_db_filter_cutoff_hz() -> f32 {
    DEFAULT_DB_FILTER_CUTOFF_HZ
}

fn default_sent_filter_enabled() -> bool {
    true
}

fn default_sent_filter_window() -> usize {
    DEFAULT_SENT_FILTER_WINDOW
}

fn default_can_autostart_tsmaster() -> bool {
    true
}

fn default_can_hardware_subtype() -> Option<i32> {
    Some(HW_SUBTYPE_TC1016)
}

fn default_can_signal_watchdog_enabled() -> bool {
    true
}

fn default_can_signal_timeout_ms() -> u64 {
    DEFAULT_CAN_SIGNAL_TIMEOUT_MS
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CollectorCanChannelConfig {
    Index(u8),
    Detailed(CollectorCanChannelDetail),
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectorCanChannelDetail {
    pub index: u8,
    #[serde(default, alias = "arbitration_baud_kbps")]
    pub baud_kbps: Option<u32>,
    #[serde(default)]
    pub data_baud_kbps: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectorCanSignalWatchdogConfig {
    pub channel: u8,
    #[serde(default, alias = "identifier")]
    pub can_id: Option<u32>,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CollectorConfig {
    pub ingress_addr: String,
    pub ui_feed_addr: String,
    pub control_addr: String,
    pub health_addr: String,
    pub can_enabled: bool,
    pub can_tsmaster_bin: Option<String>,
    #[serde(default = "default_can_autostart_tsmaster")]
    pub can_autostart_tsmaster: bool,
    pub can_hardware_name: String,
    #[serde(default = "default_can_hardware_subtype")]
    pub can_hardware_subtype: Option<i32>,
    pub can_channel: u8,
    pub can_baud_kbps: u32,
    pub can_data_baud_kbps: u32,
    #[serde(default)]
    pub can_channels: Vec<CollectorCanChannelConfig>,
    #[serde(default = "default_can_signal_watchdog_enabled")]
    pub can_signal_watchdog_enabled: bool,
    #[serde(default = "default_can_signal_timeout_ms")]
    pub can_signal_timeout_ms: u64,
    #[serde(default)]
    pub can_signal_watchdogs: Vec<CollectorCanSignalWatchdogConfig>,
    pub serial_port: Option<String>,
    pub serial_auto_detect: bool,
    pub serial_baud: u32,
    pub serial_mode: String,
    pub pg_dsn: String,
    pub pg_connect_max_retries: u32,
    pub pg_connect_retry_ms: u64,
    #[serde(default = "default_db_filter_enabled")]
    pub db_filter_enabled: bool,
    #[serde(default = "default_db_filter_order")]
    pub db_filter_order: usize,
    #[serde(default = "default_db_filter_sample_rate_hz")]
    pub db_filter_sample_rate_hz: f32,
    #[serde(default = "default_db_filter_cutoff_hz")]
    pub db_filter_cutoff_hz: f32,
    #[serde(default = "default_sent_filter_enabled")]
    pub sent_filter_enabled: bool,
    #[serde(default = "default_sent_filter_window")]
    pub sent_filter_window: usize,
    pub max_payload: usize,
    pub bus_capacity: usize,
    pub ui_feed_capacity: usize,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            ingress_addr: "127.0.0.1:19010".to_string(),
            ui_feed_addr: "127.0.0.1:19011".to_string(),
            control_addr: "127.0.0.1:19013".to_string(),
            health_addr: "127.0.0.1:19012".to_string(),
            can_enabled: false,
            can_tsmaster_bin: None,
            can_autostart_tsmaster: true,
            can_hardware_name: "TC1016".to_string(),
            can_hardware_subtype: Some(HW_SUBTYPE_TC1016),
            can_channel: 0,
            can_baud_kbps: 500,
            can_data_baud_kbps: 2_000,
            can_channels: Vec::new(),
            can_signal_watchdog_enabled: true,
            can_signal_timeout_ms: DEFAULT_CAN_SIGNAL_TIMEOUT_MS,
            can_signal_watchdogs: Vec::new(),
            serial_port: None,
            serial_auto_detect: false,
            serial_baud: 2_000_000,
            serial_mode: "sent".to_string(),
            pg_dsn: "host=127.0.0.1 port=5432 user=postgres password=123456 dbname=demo2"
                .to_string(),
            pg_connect_max_retries: 20,
            pg_connect_retry_ms: 1000,
            db_filter_enabled: false,
            db_filter_order: DEFAULT_DB_FILTER_ORDER,
            db_filter_sample_rate_hz: DEFAULT_DB_FILTER_SAMPLE_RATE_HZ,
            db_filter_cutoff_hz: DEFAULT_DB_FILTER_CUTOFF_HZ,
            sent_filter_enabled: true,
            sent_filter_window: DEFAULT_SENT_FILTER_WINDOW,
            max_payload: 4096,
            bus_capacity: 10_000,
            ui_feed_capacity: 10_000,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    collector: Option<CollectorConfig>,
}

pub fn load_collector_config() -> CollectorConfig {
    let path = "config.toml";
    let Ok(text) = fs::read_to_string(path) else {
        return CollectorConfig::default();
    };

    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    match toml::from_str::<ConfigFile>(text) {
        Ok(file) => file.collector.unwrap_or_default(),
        Err(err) => {
            warn!(error = %err, "failed to parse config.toml, fallback to defaults");
            CollectorConfig::default()
        }
    }
}

pub fn env_flag(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
}

pub fn collector_can_channels(cfg: &CollectorConfig) -> Vec<CanChannelConfig> {
    let env_arbitration_baud = std::env::var("DEMO2_COLLECTOR_CAN_BAUD_KBPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok());
    let env_data_baud = std::env::var("DEMO2_COLLECTOR_CAN_DATA_BAUD_KBPS")
        .ok()
        .and_then(|v| v.parse::<u32>().ok());
    let default_arbitration_baud = env_arbitration_baud.unwrap_or(cfg.can_baud_kbps);
    let default_data_baud = env_data_baud.unwrap_or(cfg.can_data_baud_kbps);

    if let Some(channels) = std::env::var("DEMO2_COLLECTOR_CAN_CHANNELS")
        .ok()
        .and_then(|v| parse_can_channel_list(&v))
    {
        return channels
            .into_iter()
            .map(|index| CanChannelConfig {
                index,
                arbitration_baud_kbps: default_arbitration_baud,
                data_baud_kbps: default_data_baud,
            })
            .collect();
    }

    if let Some(index) = std::env::var("DEMO2_COLLECTOR_CAN_CHANNEL")
        .ok()
        .and_then(|v| v.parse::<u8>().ok())
    {
        return vec![CanChannelConfig {
            index,
            arbitration_baud_kbps: default_arbitration_baud,
            data_baud_kbps: default_data_baud,
        }];
    }

    if cfg.can_channels.is_empty() {
        return vec![CanChannelConfig {
            index: cfg.can_channel,
            arbitration_baud_kbps: default_arbitration_baud,
            data_baud_kbps: default_data_baud,
        }];
    }

    cfg.can_channels
        .iter()
        .map(|channel| match channel {
            CollectorCanChannelConfig::Index(index) => CanChannelConfig {
                index: *index,
                arbitration_baud_kbps: default_arbitration_baud,
                data_baud_kbps: default_data_baud,
            },
            CollectorCanChannelConfig::Detailed(channel) => CanChannelConfig {
                index: channel.index,
                arbitration_baud_kbps: env_arbitration_baud
                    .or(channel.baud_kbps)
                    .unwrap_or(cfg.can_baud_kbps),
                data_baud_kbps: env_data_baud
                    .or(channel.data_baud_kbps)
                    .unwrap_or(cfg.can_data_baud_kbps),
            },
        })
        .collect()
}

fn parse_can_channel_list(value: &str) -> Option<Vec<u8>> {
    let mut channels = Vec::new();
    for item in value.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        channels.push(item.parse::<u8>().ok()?);
    }
    if channels.is_empty() {
        None
    } else {
        Some(channels)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_can_channel_list_skips_empty_items() {
        assert_eq!(parse_can_channel_list("0, 2, ,3"), Some(vec![0, 2, 3]));
        assert_eq!(parse_can_channel_list(" , "), None);
        assert_eq!(parse_can_channel_list("0,x"), None);
    }

    #[test]
    fn collector_config_accepts_bom_and_missing_fields() {
        let text = "\u{feff}[collector]\ncan_enabled = true\n";
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let file: ConfigFile = toml::from_str(text).unwrap();
        let config = file.collector.unwrap();

        assert!(config.can_enabled);
        assert_eq!(config.can_hardware_subtype, Some(HW_SUBTYPE_TC1016));
        assert!(!config.serial_auto_detect);
        assert_eq!(config.serial_baud, 2_000_000);
    }
}
