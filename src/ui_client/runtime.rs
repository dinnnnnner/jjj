use crate::{UI_QUEUE_CAPACITY, UiClientApp, UiMsg, embedded_collector_service};
use demo2::config::env_flag;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;

use super::config::{load_control_addr, load_feed_addr, load_pg_dsn};
use super::feed::{FeedStats, resilient_feed_thread};
use super::fonts::setup_chinese_fonts;

const UI_EMBED_COLLECTOR_ENV: &str = "DEMO2_UI_EMBED_COLLECTOR";

pub(crate) fn run_collector_then_ui() -> eframe::Result<()> {
    if env_flag(UI_EMBED_COLLECTOR_ENV).unwrap_or(false) {
        start_embedded_collector();
    }
    run_ui()
}

fn start_embedded_collector() {
    thread::spawn(|| {
        let rt = match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(err) => {
                eprintln!("embedded collector runtime init failed: {err}");
                return;
            }
        };

        if let Err(err) = rt.block_on(embedded_collector_service::run()) {
            eprintln!("embedded collector stopped: {err}");
        }
    });
}

fn run_ui() -> eframe::Result<()> {
    let (tx, rx) = mpsc::sync_channel::<UiMsg>(UI_QUEUE_CAPACITY);
    let feed_stats = Arc::new(FeedStats::default());
    let feed_addr = load_feed_addr();
    let control_addr = load_control_addr();
    let pg_dsn = load_pg_dsn();
    let ui_tx = tx.clone();

    let feed_stats_for_thread = feed_stats.clone();
    let feed_addr_for_thread = feed_addr.clone();
    thread::spawn(move || {
        resilient_feed_thread(feed_addr_for_thread, tx, feed_stats_for_thread);
    });

    let options = eframe::NativeOptions::default();
    let app_creator = move |cc: &eframe::CreationContext<'_>| -> Result<
        Box<dyn eframe::App>,
        Box<dyn std::error::Error + Send + Sync>,
    > {
        setup_chinese_fonts(&cc.egui_ctx);
        cc.egui_ctx
            .options_mut(|options| options.warn_on_id_clash = false);
        Ok(Box::new(UiClientApp::new(
            rx,
            ui_tx.clone(),
            feed_stats,
            feed_addr,
            control_addr,
            pg_dsn,
        )))
    };

    eframe::run_native("demo2_ui_client", options, Box::new(app_creator))
}
