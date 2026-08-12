//! CONTROL BINARY — the env-free twin of adapter_test.rs. Proves
//! AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH is load-bearing: without it, lambda_http
//! RE-PREPENDS requestContext.stage and every route 404s on the fallback — which is
//! exactly what production would do if the terraform env var were ever dropped
//! (terraform/aws-lambda.tf:85-89). No test here sets any env var; keeping this file
//! Once-free is the invariant that makes it a control.
//!
//! These fixtures do NOT go through load_v1_fixture (it lives in the flag-set binary,
//! and the stage variants intentionally break the /<stage><path> correlation the
//! predicate enforces) — they are pinned by their own assertions instead, and the
//! shared fixture (apigw_v1_link_unknown.json) is predicate-checked in the sibling
//! binary.

/// Without the flag, the stage IS prepended — this is the world terraform's env var
/// prevents. Same fixture as the flag-set binary, opposite verdict: the pair proves
/// the env var is load-bearing rather than decorative (spec D2/D3).
#[tokio::test]
async fn without_stage_flag_the_stage_is_prepended_and_routing_breaks() {
    assert!(
        std::env::var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH").is_err(),
        "control precondition: flag must be ABSENT in this binary — if this fires, \
         something leaked ambient env (a .cargo [env] entry?) and the control is VOID, \
         not failed"
    );
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_link_unknown.json"))
        .expect("fixture must still translate");
    assert_eq!(
        req.uri().path(),
        "/live/api/l/definitely-not-a-token",
        "without the flag, stage IS prepended — this is the world terraform's env var prevents"
    );
}

/// The stage-ABSENT arm (spec D2, plan-review M4): with no stage in the event
/// (requestContext.stage null), the path is not prefixed even without the flag —
/// request.rs's `None => path.into()` branch, otherwise unreachable in these suites.
#[tokio::test]
async fn without_stage_in_event_the_path_is_untouched_even_without_flag() {
    assert!(
        std::env::var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH").is_err(),
        "control precondition: flag must be ABSENT in this binary"
    );
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_no_stage.json"))
        .expect("fixture must translate");
    assert_eq!(
        req.uri().path(),
        "/api/l/definitely-not-a-token",
        "no stage in event ⇒ nothing to prepend, flag or no flag"
    );
}

/// `$default` stage arm (spec AC1's fourth arm; OMBB step-5 M1): `Some("$default")`
/// and `None` are DISTINCT match arms in apigw_path_with_stage — each deserves its
/// own pin. `$default` is a sentinel, not a prefix.
#[tokio::test]
async fn default_stage_is_never_prepended_even_without_flag() {
    assert!(
        std::env::var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH").is_err(),
        "control precondition: flag must be ABSENT in this binary"
    );
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_default_stage.json"))
        .expect("fixture must translate");
    assert_eq!(
        req.uri().path(),
        "/api/l/definitely-not-a-token",
        "$default is a sentinel, not a prefix"
    );
}
