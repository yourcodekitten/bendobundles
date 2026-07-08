//! Run-once STEAMAPP# cache rebuild (issue #57): refetches appdetails for every catalog
//! appid through the current parse (id-allowlisted genres) and rewrites each item,
//! preserving the reviews half. Run by a human with AWS credentials, never by CI or the
//! lambda:
//!
//!   TABLE_NAME=<table> cargo run -p fulfillment --features backfill --bin backfill_genres
//!
//! Optional env: SKIP_FRESH_SECS (default 43200 = 12h) — items whose appdetails were
//! fetched more recently than this are skipped, which is what makes a rerun after a 429
//! abort resume where it left off.
//!
//! Paced at STEAM_ENRICH_PACE (1.5s/app): the ~700-app catalog takes ~18 minutes.
use dynamo::Store;
use steam_client::{SteamApiKey, SteamClient};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().init();
    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME required");
    let skip_fresh_secs: i64 = std::env::var("SKIP_FRESH_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(43_200);
    let aws_cfg = aws_config::load_from_env().await;
    let store = Store::new(aws_sdk_dynamodb::Client::new(&aws_cfg), table);
    // The appdetails storefront endpoint is keyless; no web-API call is made, so an
    // empty key is fine here.
    let steam = SteamClient::new(
        "https://api.steampowered.com",
        "https://store.steampowered.com",
        "https://steamcommunity.com",
        SteamApiKey::new(String::new()),
    )
    .expect("SteamClient construction");
    let summary = fulfillment::backfill_steam_genres(
        &store,
        &steam,
        fulfillment::STEAM_ENRICH_PACE,
        skip_fresh_secs,
    )
    .await
    .expect("backfill: list_all_games failed");
    println!(
        "backfill: fetched={} negative={} skipped={} failed={} aborted_429={}",
        summary.fetched, summary.negative, summary.skipped, summary.failed, summary.aborted_429
    );
    if summary.aborted_429 {
        eprintln!("rate-limited — rerun to resume (items already rewritten are skipped)");
        std::process::exit(2);
    }
}
