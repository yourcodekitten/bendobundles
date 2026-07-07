use std::sync::Arc;

use public_api::{Invoker, LambdaInvoker, router};
use steam_client::{SteamApiKey, SteamClient};

#[tokio::main]
async fn main() {
    // Send tracing to stdout → CloudWatch. Without this the claim path is a
    // black box in prod (the lambda emits only runtime START/END lines).
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .init();

    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME must be set");
    let fn_name = std::env::var("FULFILLMENT_FN").expect("FULFILLMENT_FN must be set");
    let base_url = std::env::var("BASE_URL").expect("BASE_URL must be set");

    let cfg = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    let store = Arc::new(dynamo::Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        table,
    ));

    let invoker: Arc<dyn Invoker> = Arc::new(LambdaInvoker {
        client: aws_sdk_lambda::Client::new(&cfg),
        fn_name,
    });

    // Steam client is optional — if STEAM_API_KEY is unset, steam endpoints return 503.
    let steam = std::env::var("STEAM_API_KEY").ok().map(|key| {
        Arc::new(
            SteamClient::new(
                "https://api.steampowered.com",
                "https://store.steampowered.com",
                "https://steamcommunity.com",
                SteamApiKey::new(key),
            )
            .expect("failed to build SteamClient"),
        )
    });

    lambda_http::run(router(store, invoker, steam, base_url))
        .await
        .expect("lambda_http run failed");
}
