use dynamo::Store;
use fulfillment::{
    Deps, FulfillRequest, FulfillResponse, Notify, SecretRead, SessionStore,
    compute_enrich_deadline, handle,
};
use humble_client::{HumbleClient, SessionCookie, StepUpCredentials};
use lambda_runtime::{LambdaEvent, service_fn};
use steam_client::SteamClient;

/// Read one decrypted SSM SecureString and say **what was found**, not merely whether a value came
/// back.
///
/// This used to return `Option<String>`, collapsing FOUR states into one `None`: parameter absent,
/// the `"UNSET"` placeholder, an empty value, and an SSM/KMS/IAM **error**. The fourth is transient
/// external weather being recorded as permanent internal intent — and because config resolves once
/// per container, a momentary throttle became a container-lifetime condition. See `SecretRead`.
///
/// `"UNSET"` and empty are `DeliberatelyOff`, not values: terraform seeds these params with the
/// placeholder, so a param that exists but was never given a real value out-of-band must read as
/// unconfigured and NOT as a credential. Without that, `Some("UNSET")` would attach as the password
/// and every gated redeem would POST a wrong password + bogus TOTP at the live account (lockout /
/// rate-limit risk). The value is a secret: never logged, only the param NAME.
///
/// (The previous doc-comment's own summary line read *"Returns `None` (with a warn) on any error"* —
/// **a precise description of the four-state collapse, left standing as the first thing a reader
/// sees, directly above the code written to remove it.** A stale doc does not merely fail to help;
/// it actively teaches the contract that was just retired.)
async fn get_secret(client: &aws_sdk_ssm::Client, param: &str) -> SecretRead {
    match client
        .get_parameter()
        .name(param)
        .with_decryption(true)
        .send()
        .await
    {
        Ok(out) => match out.parameter().and_then(|p| p.value()) {
            Some(v) if !v.is_empty() && v != "UNSET" => SecretRead::Resolved(v.to_string()),
            _ => SecretRead::DeliberatelyOff,
        },
        Err(e) => {
            tracing::warn!(error = %e, param, "SSM get_parameter (secret) failed");
            SecretRead::ReadFailed
        }
    }
}

/// For callers that genuinely only need "a value or nothing" — the cookie/password/TOTP paths.
/// Explicit, so collapsing the distinction is a DECISION at the call site rather than a default.
fn secret_value(r: SecretRead) -> Option<String> {
    match r {
        SecretRead::Resolved(v) => Some(v),
        _ => None,
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

    // Ben's be-nice kill switch: STEAM_ENRICH_DISABLED=1 skips the storefront enrichment pass.
    // Read once at startup; carried on Deps so run_sync's enrichment reads config, not raw env.
    let steam_enrich_disabled = std::env::var("STEAM_ENRICH_DISABLED").as_deref() == Ok("1");
    tracing::info!(steam_enrich_disabled, "steam enrichment configuration");

    // Retry PINNED, not inherited. The SDK default (standard, 3 attempts) already applies — but an
    // inherited default is a property of a VERSION, not of this code, and it disappears in a
    // refactor with nothing announcing it. The ReadFailed design leans on retry converting the
    // transient-weather population into Resolved BEFORE it can become a container-lifetime
    // condition, so the pin is load-bearing rather than tidy.
    let aws_cfg = aws_config::from_env()
        .retry_config(aws_config::retry::RetryConfig::standard())
        .load()
        .await;
    let dynamo_client = aws_sdk_dynamodb::Client::new(&aws_cfg);
    let ssm_client = aws_sdk_ssm::Client::new(&aws_cfg);
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5)) // ping is fire-and-forget; a hung webhook must not stall the run
        .build()
        .expect("reqwest client");

    let steam_key = match &steam_key_param {
        Some(param) => secret_value(get_secret(&ssm_client, param).await),
        None => None,
    };
    let steam = SteamClient::configure(steam_key);

    // Webhook URL fetched ONCE at startup — cache it. The param is a SecureString (a webhook
    // URL is itself the post-access credential, #81) seeded with the UNSET placeholder until an
    // operator PutParameters the real value: get_secret decrypts, reads UNSET/empty/error as
    // webhooks-off, and never logs the value. Never crash over it.
    let webhook_read: SecretRead = if let Some(ref param) = webhook_param {
        get_secret(&ssm_client, param).await
    } else {
        SecretRead::DeliberatelyOff
    };
    let notify_disabled = std::env::var("NOTIFY_DISABLED").as_deref() == Ok("1");
    let notify = Notify::resolve(webhook_read, notify_disabled);

    // The WHISPER register — a second webhook param, same SecureString/UNSET machinery, resolved
    // ONCE at startup like the ops one. Suppression flag is WHISPER_DISABLED, NEVER the global
    // NOTIFY_DISABLED: reusing the global would re-couple the registers the second param exists
    // to separate (quieting ops must not silently kill the gift feature).
    let whisper_param = std::env::var("WHISPER_WEBHOOK_PARAM").ok();
    let whisper_read: SecretRead = if let Some(ref param) = whisper_param {
        get_secret(&ssm_client, param).await
    } else {
        SecretRead::DeliberatelyOff
    };
    let whisper_disabled = std::env::var("WHISPER_DISABLED").as_deref() == Ok("1");
    let whisper_notify = Notify::resolve(whisper_read, whisper_disabled);
    // Carried for the DARK announcement's one-liner: the message must always name something
    // actionable, so an unwired env gets a literal saying exactly that.
    let whisper_param_name =
        whisper_param.unwrap_or_else(|| "(WHISPER_WEBHOOK_PARAM env unset)".to_string());
    let whisper_site_url =
        std::env::var("WHISPER_SITE_URL").unwrap_or_else(|_| "https://bendobundles.com".into());
    match notify {
        // LOUD, not CLOSED. This structured line is the metric-filter/alarm target: it pages
        // "this deploy is running with notifications unresolvable" WITHOUT coupling fulfilment to
        // monitoring. The process continues; orders are never held hostage to a missing var.
        Notify::Unresolved => tracing::error!(
            outcome = "notify_unresolved",
            "operator notifications UNRESOLVABLE — running blind; fulfilment continues"
        ),
        Notify::Disabled => tracing::warn!(
            outcome = "notify_disabled",
            "operator notifications are off (absent/UNSET param, or NOTIFY_DISABLED=1)"
        ),
        Notify::Webhook(_) => {}
    }

    lambda_runtime::run(service_fn(|event: LambdaEvent<serde_json::Value>| {
        // Clone cheap Arc-backed handles; reconstruct Store per-invoke (not Clone).
        let dynamo_client = dynamo_client.clone();
        let ssm_client = ssm_client.clone();
        let http_client = http_client.clone();
        let table = table.clone();
        let cookie_param = cookie_param.clone();
        let notify = notify.clone();
        let whisper_notify = whisper_notify.clone();
        let whisper_param_name = whisper_param_name.clone();
        let whisper_site_url = whisper_site_url.clone();
        let base_url = base_url.clone();
        let step_up_username = step_up_username.clone();
        let password_param = password_param.clone();
        let totp_param = totp_param.clone();
        let steam = steam.clone();

        async move {
            // Compute the enrichment deadline from the lambda context's per-invoke remaining time.
            // context.deadline is epoch-ms; now_epoch_ms is the wall clock at invocation start.
            let now_epoch_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let steam_enrich_deadline = tokio::time::Instant::now()
                + compute_enrich_deadline(event.context.deadline, now_epoch_ms);
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
                            async { secret_value(get_secret(&ssm_client, pw_param).await) },
                            async { secret_value(get_secret(&ssm_client, totp_p).await) },
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
                    notify,
                    whisper_notify,
                    whisper_site_url,
                    whisper_param_name,
                    http: http_client,
                    session_store,
                    steam: steam.clone(),
                    steam_enrich_disabled,
                    steam_enrich_pace: fulfillment::STEAM_ENRICH_PACE,
                    steam_enrich_deadline,
                    choice_discovery_deadline: std::time::Duration::from_secs(180),
                };

                handle(&deps, req).await
            };

            Ok::<_, lambda_runtime::Error>(response)
        }
    }))
    .await
}
