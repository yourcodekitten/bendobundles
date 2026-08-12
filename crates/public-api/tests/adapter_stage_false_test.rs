//! THIRD ENV WORLD (spec D2): AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH set to the string
//! "false". lambda_http checks PRESENCE (request.rs:408 `env::var(...).is_ok()`), so
//! "false" still strips the stage — terraform's `= "true"` works by presence, not truth.
//! If this test ever fails, lambda_http moved to value-semantics and terraform's config
//! (and both sibling binaries' assumptions) must be re-read.

use std::sync::Once;

static STAGE_ENV: Once = Once::new();

fn init_false_env() {
    STAGE_ENV.call_once(|| {
        // SAFETY: same containment contract as adapter_test.rs's init (spec D2).
        unsafe { std::env::set_var("AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH", "false") }
    });
}

#[tokio::test]
async fn flag_set_to_false_still_strips_the_stage() {
    init_false_env();
    let req = lambda_http::request::from_str(include_str!("fixtures/apigw_v1_link_unknown.json"))
        .expect("fixture must translate");
    assert_eq!(
        req.uri().path(),
        "/api/l/definitely-not-a-token",
        "the flag is PRESENCE-triggered: \"false\" activates it too"
    );
}
