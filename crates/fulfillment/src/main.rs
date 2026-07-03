use dynamo::Store;
use fulfillment::{Deps, FulfillRequest, FulfillResponse, handle};
use humble_client::{HumbleClient, SessionCookie};
use lambda_runtime::{LambdaEvent, service_fn};

#[tokio::main]
async fn main() -> Result<(), lambda_runtime::Error> {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .init();

    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME required");
    let cookie_param = std::env::var("HUMBLE_COOKIE_PARAM").expect("HUMBLE_COOKIE_PARAM required");
    let webhook_param = std::env::var("DISCORD_WEBHOOK_PARAM").ok();
    let base_url =
        std::env::var("BASE_URL").unwrap_or_else(|_| "https://www.humblebundle.com".into());

    let aws_cfg = aws_config::load_from_env().await;
    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_cfg);
    let ssm_client = aws_sdk_ssm::Client::new(&aws_cfg);
    let http_client = reqwest::Client::new();

    // Webhook URL fetched ONCE at startup — non-secret, cache it. On missing/failed param, warn
    // and continue without webhooks; never crash.
    let webhook_url: Option<String> = if let Some(ref param) = webhook_param {
        match ssm_client.get_parameter().name(param).send().await {
            Ok(out) => out.parameter().and_then(|p| p.value()).map(str::to_string),
            Err(e) => {
                tracing::warn!(error = %e, param, "discord webhook param fetch failed; webhooks disabled");
                None
            }
        }
    } else {
        None
    };

    lambda_runtime::run(service_fn(|event: LambdaEvent<serde_json::Value>| {
        // Clone cheap Arc-backed handles; reconstruct Store per-invoke (not Clone).
        let dynamo_client = dynamo_client.clone();
        let ssm_client = ssm_client.clone();
        let http_client = http_client.clone();
        let table = table.clone();
        let cookie_param = cookie_param.clone();
        let webhook_url = webhook_url.clone();
        let base_url = base_url.clone();

        async move {
            let payload = event.payload;

            // Try to parse as a typed request; on failure fall back to EventBridge → Sync.
            let response: FulfillResponse = 'dispatch: {
                let req = if let Ok(r) = serde_json::from_value::<FulfillRequest>(payload.clone())
                {
                    r
                } else if payload.get("source").and_then(|v| v.as_str()) == Some("aws.events") {
                    // eventbridge schedule → sync
                    FulfillRequest::Sync
                } else {
                    break 'dispatch FulfillResponse::Error {
                        message: "unrecognized invocation payload".into(),
                    };
                };

                // Per-invoke SSM cookie fetch — freshness beats latency; an admin paste takes
                // effect on the very next claim, no warm-container staleness.
                let cookie_value = match ssm_client
                    .get_parameter()
                    .name(&cookie_param)
                    .with_decryption(true)
                    .send()
                    .await
                {
                    Ok(out) => match out.parameter().and_then(|p| p.value()).map(str::to_string) {
                        Some(v) => v,
                        None => {
                            tracing::error!(param = %cookie_param, "SSM parameter returned no value");
                            break 'dispatch FulfillResponse::Error {
                                message: "humble session unavailable".into(),
                            };
                        }
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "SSM get_parameter failed");
                        break 'dispatch FulfillResponse::Error {
                            message: "humble session unavailable".into(),
                        };
                    }
                };

                let humble = match HumbleClient::new(&base_url, SessionCookie::new(cookie_value)) {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::error!(error = %e, "HumbleClient construction failed");
                        break 'dispatch FulfillResponse::Error {
                            message: "humble session unavailable".into(),
                        };
                    }
                };

                let deps = Deps {
                    store: Store::new(dynamo_client, table),
                    humble,
                    webhook_url,
                    http: http_client,
                };

                handle(&deps, req).await
            };

            Ok::<_, lambda_runtime::Error>(response)
        }
    }))
    .await
}
