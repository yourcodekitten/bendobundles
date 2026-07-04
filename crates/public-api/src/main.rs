use std::sync::Arc;

use public_api::{Invoker, LambdaInvoker, router};

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

    let cfg = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;

    let store = Arc::new(dynamo::Store::new(
        aws_sdk_dynamodb::Client::new(&cfg),
        table,
    ));

    let invoker: Arc<dyn Invoker> = Arc::new(LambdaInvoker {
        client: aws_sdk_lambda::Client::new(&cfg),
        fn_name,
    });

    lambda_http::run(router(store, invoker))
        .await
        .expect("lambda_http run failed");
}
