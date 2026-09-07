//! S3Template integration tests against moto.
//!
//! Contract (BLOCKER-1, 2026-09-07 final review): this suite reads its OWN env
//! var, `S3_LOCAL_URL` — never `DYNAMODB_LOCAL_URL`. CI's dynamo suites run
//! against `amazon/dynamodb-local`, which implements ONLY the DynamoDB API and
//! does NOT answer S3; reusing that var here made both tests here guaranteed-red
//! in CI while reading green on every local box that happens to run moto (moto's
//! server mode DOES answer every AWS service on one endpoint, so a local moto at
//! `DYNAMODB_LOCAL_URL` masked the gap completely). CI now runs a dedicated
//! `motoserver/moto` service and points `S3_LOCAL_URL` at it.
//!
//! `s3_or_skip` mirrors `crates/dynamo`'s `store_or_skip` pattern: env var
//! explicitly set ⇒ the endpoint MUST work, panic loudly on failure (refusing to
//! forge a green run); env var unset ⇒ skip quietly with an eprintln naming
//! `S3_LOCAL_URL`, so a local run without moto up doesn't fail, but a CI run
//! (which always sets it) can never silently skip.
use public_api::{S3Template, TemplateSource};

async fn s3_or_skip(test: &str) -> Option<(aws_sdk_s3::Client, String)> {
    let (url, explicit) = match std::env::var("S3_LOCAL_URL") {
        Ok(v) => (v, true),
        Err(_) => ("http://localhost:8000".into(), false),
    };
    let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .endpoint_url(&url)
        .region("us-east-1")
        .test_credentials()
        .load()
        .await;
    // force_path_style: moto's S3 emulation expects `http://host:port/bucket/key`,
    // not virtual-hosted-style `http://bucket.host:port/key`.
    let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
        .force_path_style(true)
        .build();
    let client = aws_sdk_s3::Client::from_conf(s3_config);
    if client.list_buckets().send().await.is_err() {
        if explicit {
            panic!(
                "S3_LOCAL_URL is set but moto is unreachable — \
                 refusing to skip (this would forge a green run)"
            );
        }
        eprintln!("SKIP {test}: S3_LOCAL_URL not set and no moto/s3-local at {url}");
        return None;
    }
    let bucket = format!("t-unfurl-{test}");
    client.create_bucket().bucket(&bucket).send().await.unwrap();
    Some((client, bucket))
}

const TPL: &str =
    "<html><head>\n<!-- og:begin -->\nGENERIC\n<!-- og:end -->\n</head><body></body></html>";

#[tokio::test]
async fn s3_template_fetches_deployed_index_html() {
    let Some((client, bucket)) = s3_or_skip("fetch").await else {
        return;
    };
    client
        .put_object()
        .bucket(&bucket)
        .key("index.html")
        .body(TPL.as_bytes().to_vec().into())
        .send()
        .await
        .unwrap();

    let source = S3Template::new(client, bucket);
    let fetched = source.fetch().await.unwrap();
    assert_eq!(fetched, TPL);
}

/// Pins the 60s cache semantics (spec: docs/spec-wrapping-paper.md D3): a
/// `put_object` overwrite that lands within the TTL must NOT be visible on the
/// next `fetch()` — the cache is real, not an accidental single-call memo.
#[tokio::test]
async fn s3_template_serves_stale_copy_within_ttl_after_overwrite() {
    let Some((client, bucket)) = s3_or_skip("cache-ttl").await else {
        return;
    };
    client
        .put_object()
        .bucket(&bucket)
        .key("index.html")
        .body(TPL.as_bytes().to_vec().into())
        .send()
        .await
        .unwrap();

    let source = S3Template::new(client.clone(), bucket.clone());
    let first = source.fetch().await.unwrap();
    assert_eq!(first, TPL);

    let changed = "<html><head>\n<!-- og:begin -->\nCHANGED\n<!-- og:end -->\n</head></html>";
    client
        .put_object()
        .bucket(&bucket)
        .key("index.html")
        .body(changed.as_bytes().to_vec().into())
        .send()
        .await
        .unwrap();

    // Same S3Template instance, fetched immediately — well within the 60s TTL —
    // must still hand back the FIRST bytes it cached.
    let second = source.fetch().await.unwrap();
    assert_eq!(
        second, TPL,
        "a fetch within the 60s TTL must serve the cached copy, not the fresh overwrite"
    );
    assert_ne!(second, changed);
}
