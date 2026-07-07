use std::sync::Arc;

use public_api::{Invoker, LambdaInvoker, router};
use steam_client::{SteamApiKey, SteamClient};

async fn get_secret(client: &aws_sdk_ssm::Client, param: &str) -> Option<String> {
    match client
        .get_parameter()
        .name(param)
        .with_decryption(true)
        .send()
        .await
    {
        Ok(out) => out
            .parameter()
            .and_then(|p| p.value())
            .filter(|v| !v.is_empty() && *v != "UNSET")
            .map(str::to_string),
        Err(e) => {
            tracing::warn!(error = %e, param, "SSM get_parameter (secret) failed");
            None
        }
    }
}

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
    let steam_key_param = std::env::var("STEAM_KEY_PARAM").ok();

    let cfg = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    let store = Arc::new(dynamo::Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        table,
    ));

    let invoker: Arc<dyn Invoker> = Arc::new(LambdaInvoker {
        client: aws_sdk_lambda::Client::new(&cfg),
        fn_name,
    });

    let ssm_client = aws_sdk_ssm::Client::new(&cfg);
    let steam: Option<Arc<SteamClient>> = if let Some(ref param) = steam_key_param {
        match get_secret(&ssm_client, param).await {
            Some(key) => match SteamClient::new(
                "https://api.steampowered.com",
                "https://store.steampowered.com",
                "https://steamcommunity.com",
                SteamApiKey::new(key),
            ) {
                Ok(c) => {
                    tracing::info!("steam client: configured");
                    Some(Arc::new(c))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "SteamClient construction failed");
                    tracing::info!("steam client: absent");
                    None
                }
            },
            None => {
                tracing::info!("steam client: absent");
                None
            }
        }
    } else {
        tracing::info!("steam client: absent");
        None
    };

    lambda_http::run(router(store, invoker, steam, base_url))
        .await
        .expect("lambda_http run failed");
}
