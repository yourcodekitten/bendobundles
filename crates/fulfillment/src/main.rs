use std::sync::Arc;

use dynamo::Store;
use fulfillment::{Deps, FulfillRequest, FulfillResponse, SessionStore, handle};
use humble_client::{HumbleClient, SessionCookie, StepUpCredentials};
use lambda_runtime::{LambdaEvent, service_fn};
use steam_client::{SteamApiKey, SteamClient};

/// Fetch one decrypted SSM SecureString. Returns `None` (with a warn) on any error, an empty value,
/// or the `"UNSET"` placeholder that terraform seeds these containers with — so a param that exists
/// but was never given a real value out-of-band reads as unconfigured, NOT as a credential. Without
/// this, `Some("UNSET")` would attach as the password and every gated redeem would POST a wrong
/// password + bogus TOTP to the live account (lockout / rate-limit risk). The value is a secret:
/// never logged, only the param NAME.
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

    // Secure-area step-up config (all three required to enable it). When any is unset, the client
    // is built WITHOUT step-up and a gated redeem parks exactly as before — a safe, opt-in default.
    // Username is a plain env var (account-identifying, not a secret); password + TOTP seed are SSM
    // SecureStrings fetched per-invoke alongside the cookie.
    let step_up_username = std::env::var("HUMBLE_USERNAME").ok();
    let password_param = std::env::var("HUMBLE_PASSWORD_PARAM").ok();
    let totp_param = std::env::var("HUMBLE_TOTP_PARAM").ok();
    let step_up_enabled =
        step_up_username.is_some() && password_param.is_some() && totp_param.is_some();
    tracing::info!(step_up_enabled, "secure-area step-up configuration");

    let steam_key_param = std::env::var("STEAM_KEY_PARAM").ok();

    let aws_cfg = aws_config::load_from_env().await;
    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_cfg);
    let ssm_client = aws_sdk_ssm::Client::new(&aws_cfg);
    let http_client = reqwest::Client::new();

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
        let step_up_username = step_up_username.clone();
        let password_param = password_param.clone();
        let totp_param = totp_param.clone();
        let steam = steam.clone();

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

                // Per-invoke SSM cookie fetch — freshness beats latency; a self-login persist
                // (or a manual SSM update) takes effect on the very next claim, no
                // warm-container staleness.
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

                // Attach the humble credentials whenever configured AND both secrets resolve. Needed
                // on EVERY op now: the client uses them for the secure-area step-up AND for
                // self-login, so validate/sync — and the gift path, in-line — can self-heal a dead
                // session with no human cookie paste. A fetch miss is
                // non-fatal: the client still works, and a dead session or gated redeem just parks.
                // Yield the client + its session_store together, so "creds resolved ⇒ can persist a
                // self-login" is decided in one place (no separate derived bool to keep in sync).
                // session_store is Some only when we have credentials to log in with; otherwise a
                // dead session falls back to the old flag-and-ping.
                let (humble, session_store) = match (&step_up_username, &password_param, &totp_param)
                {
                    (Some(username), Some(pw_param), Some(totp_p)) => {
                        // The two fetches are independent and run on every invoke (including the
                        // synchronous admin-validate and friend-facing gift paths) — overlap them.
                        match tokio::join!(
                            get_secret(&ssm_client, pw_param),
                            get_secret(&ssm_client, totp_p),
                        ) {
                            (Some(password), Some(totp_secret)) => (
                                humble.with_step_up(StepUpCredentials::new(
                                    username.clone(),
                                    password,
                                    totp_secret,
                                )),
                                Some(SessionStore {
                                    ssm: ssm_client.clone(),
                                    cookie_param: cookie_param.clone(),
                                }),
                            ),
                            _ => {
                                tracing::warn!(
                                    "humble credentials configured but a secret param did not resolve — proceeding without step-up/self-login"
                                );
                                (humble, None)
                            }
                        }
                    }
                    _ => (humble, None),
                };

                let deps = Deps {
                    store: Store::new(dynamo_client, table),
                    humble,
                    webhook_url,
                    http: http_client,
                    session_store,
                    steam: steam.clone(),
                };

                handle(&deps, req).await
            };

            Ok::<_, lambda_runtime::Error>(response)
        }
    }))
    .await
}
