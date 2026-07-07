//! admin-api lambda entry point.
//!
//! Env vars (all required):
//! - `TABLE_NAME`          — DynamoDB table name
//! - `FULFILLMENT_FN`      — fulfillment lambda function name/ARN
//! - `ADMIN_HASH_PARAM`    — SSM parameter name for the argon2 admin password hash (SecureString)
//! - `STEAM_KEY_PARAM`     — SSM parameter name for the Steam Web API key (optional; absent → steam off)
//!
//! Startup: SSM `GetParameter` (with_decryption=true) for `ADMIN_HASH_PARAM` loads the argon2
//! PHC string. Any failure here panics — the lambda must not start without the hash.
use std::sync::Arc;

use admin_api::{AdminInvoker, router};
use async_trait::async_trait;
use fulfillment::{FulfillRequest, FulfillResponse};
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

// ── Real AdminInvoker ─────────────────────────────────────────────────────────

struct RealAdminInvoker {
    client: aws_sdk_lambda::Client,
    fn_name: String,
}

#[async_trait]
impl AdminInvoker for RealAdminInvoker {
    async fn fire(&self, req: FulfillRequest) -> Result<(), String> {
        let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        // Event = async invoke: returns once the request is queued, does NOT
        // wait for the handler. A full backfill runs for minutes; awaiting it
        // through the API Gateway request path 504s.
        self.client
            .invoke()
            .function_name(&self.fn_name)
            .invocation_type(aws_sdk_lambda::types::InvocationType::Event)
            .payload(aws_sdk_lambda::primitives::Blob::new(payload))
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
    }

    async fn call(&self, req: FulfillRequest) -> Result<FulfillResponse, String> {
        let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        let resp = self
            .client
            .invoke()
            .function_name(&self.fn_name)
            .invocation_type(aws_sdk_lambda::types::InvocationType::RequestResponse)
            .payload(aws_sdk_lambda::primitives::Blob::new(payload))
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        let blob = resp
            .payload()
            .ok_or_else(|| "no payload in lambda response".to_string())?;
        serde_json::from_slice(blob.as_ref()).map_err(|e| e.to_string())
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .init();

    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME must be set");
    let fn_name = std::env::var("FULFILLMENT_FN").expect("FULFILLMENT_FN must be set");
    let admin_hash_param = std::env::var("ADMIN_HASH_PARAM").expect("ADMIN_HASH_PARAM must be set");
    let steam_key_param = std::env::var("STEAM_KEY_PARAM").ok();

    let cfg = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    // Load the admin password hash from SSM at boot. Panic on failure — the lambda must not
    // start without a valid hash.
    let ssm_client = aws_sdk_ssm::Client::new(&cfg);
    let admin_hash = ssm_client
        .get_parameter()
        .name(&admin_hash_param)
        .with_decryption(true)
        .send()
        .await
        .expect("failed to load ADMIN_HASH_PARAM from SSM at startup")
        .parameter()
        .and_then(|p| p.value())
        .expect("ADMIN_HASH_PARAM exists in SSM but has no value")
        .to_string();

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

    let store = Arc::new(dynamo::Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        table,
    ));

    let invoker: Arc<dyn AdminInvoker> = Arc::new(RealAdminInvoker {
        client: aws_sdk_lambda::Client::new(&cfg),
        fn_name,
    });

    lambda_http::run(router(store, invoker, admin_hash, steam))
        .await
        .expect("lambda_http::run failed");
}
