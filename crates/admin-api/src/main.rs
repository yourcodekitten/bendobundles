//! admin-api lambda entry point.
//!
//! Env vars (all required):
//! - `TABLE_NAME`          — DynamoDB table name
//! - `FULFILLMENT_FN`      — fulfillment lambda function name/ARN
//! - `ADMIN_HASH_PARAM`    — SSM parameter name for the argon2 admin password hash (SecureString)
//! - `HUMBLE_COOKIE_PARAM` — SSM parameter name where humble cookie is written by `POST /cookie`
//!
//! Startup: SSM `GetParameter` (with_decryption=true) for `ADMIN_HASH_PARAM` loads the argon2
//! PHC string. Any failure here panics — the lambda must not start without the hash.
use std::sync::Arc;

use admin_api::{AdminInvoker, SsmPutter, router};
use async_trait::async_trait;
use fulfillment::{FulfillRequest, FulfillResponse};

// ── Real AdminInvoker ─────────────────────────────────────────────────────────

struct RealAdminInvoker {
    client: aws_sdk_lambda::Client,
    fn_name: String,
}

#[async_trait]
impl AdminInvoker for RealAdminInvoker {
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

// ── Real SsmPutter ────────────────────────────────────────────────────────────

struct RealSsmPutter {
    client: aws_sdk_ssm::Client,
    param_name: String,
}

#[async_trait]
impl SsmPutter for RealSsmPutter {
    async fn put_cookie(&self, value: &str) -> Result<(), String> {
        // Overwrite the SecureString parameter with the new cookie value.
        // SECURITY: `value` must not be logged here or anywhere up the call stack.
        self.client
            .put_parameter()
            .name(&self.param_name)
            .value(value)
            .r#type(aws_sdk_ssm::types::ParameterType::SecureString)
            .overwrite(true)
            .send()
            .await
            .map_err(|e| format!("{e:?}"))?;
        Ok(())
    }

    async fn get_cookie(&self) -> Result<Option<String>, String> {
        // SECURITY: the returned value is the humble session cookie — never log or echo it.
        match self
            .client
            .get_parameter()
            .name(&self.param_name)
            .with_decryption(true)
            .send()
            .await
        {
            Ok(out) => Ok(out.parameter().and_then(|p| p.value()).map(str::to_string)),
            Err(e) => {
                // ParameterNotFound is a legitimate "no prior value" state — return Ok(None).
                if e.as_service_error()
                    .map(|se| {
                        matches!(
                            se,
                            aws_sdk_ssm::operation::get_parameter::GetParameterError::ParameterNotFound(_)
                        )
                    })
                    .unwrap_or(false)
                {
                    Ok(None)
                } else {
                    Err(format!("{e:?}"))
                }
            }
        }
    }
}

// ── main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let table = std::env::var("TABLE_NAME").expect("TABLE_NAME must be set");
    let fn_name = std::env::var("FULFILLMENT_FN").expect("FULFILLMENT_FN must be set");
    let admin_hash_param = std::env::var("ADMIN_HASH_PARAM").expect("ADMIN_HASH_PARAM must be set");
    let cookie_param =
        std::env::var("HUMBLE_COOKIE_PARAM").expect("HUMBLE_COOKIE_PARAM must be set");

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

    let store = Arc::new(dynamo::Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        table,
    ));

    let invoker: Arc<dyn AdminInvoker> = Arc::new(RealAdminInvoker {
        client: aws_sdk_lambda::Client::new(&cfg),
        fn_name,
    });

    let ssm: Arc<dyn SsmPutter> = Arc::new(RealSsmPutter {
        client: ssm_client,
        param_name: cookie_param,
    });

    lambda_http::run(router(store, invoker, ssm, admin_hash))
        .await
        .expect("lambda_http::run failed");
}
