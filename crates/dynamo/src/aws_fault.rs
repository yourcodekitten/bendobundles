//! A DynamoDB failure reduced to what diagnoses it — and nothing that can carry item data.
//!
//! # The rule this type exists to enforce
//!
//! The modeled `.message()` is **IN**. `.item()` is **NEVER**.
//!
//! [`StoreError::Aws`](crate::StoreError::Aws) used to hold `format!("{e:?}")` — an *unbounded*
//! capture of a `Debug` we do not own. `SdkError` is `#[non_exhaustive]` and **four of its five
//! arms** carry an unbounded payload: `ConstructionFailure` and `TimeoutError` hold a `BoxError`,
//! `DispatchFailure` holds a `ConnectorError` that is itself a `BoxError`, and `ResponseError`
//! holds a `BoxError` beside the raw response. `ServiceError` — the modeled one — is the only
//! bounded arm. So whatever the SDK boxed, we adopted, into a value that reaches the operator
//! Discord channel. This type bounds that capture.
//!
//! Verified against `aws-smithy-runtime-api` 1.14.0, the version `Cargo.lock` resolves.
//!
//! Nothing was leaking when this was written; see
//! `docs/superpowers/specs/2026-08-21-sealed-by-construction-design.md` for the honesty section.

// Re-exported by the SDK, so this needs no new dependency: `aws_smithy_types` is only a
// DEV-dependency of this crate (the first draft imported it in non-test code and would not
// compile).
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::ProvideErrorMetadata;

/// A bounded description of a failed DynamoDB call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsFault {
    op: &'static str,
    code: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
    http_status: Option<u16>,
    retryable: bool,
}

impl AwsFault {
    /// Extract the diagnosable parts of an SDK error.
    ///
    /// Deliberately takes `&SdkError` so callers keep the original for typed classification
    /// (`is_ccf_put` and friends run on the borrowed error, upstream of this conversion).
    /// `R` is pinned to the SDK's own [`HttpResponse`] rather than left generic: every real
    /// call site in this crate uses it, and a generic `R` has no `.status()` to read — which
    /// is how the first draft failed to compile.
    pub fn from_sdk_error<E>(
        op: &'static str,
        e: &aws_sdk_dynamodb::error::SdkError<E, HttpResponse>,
    ) -> Self
    where
        E: ProvideErrorMetadata + std::error::Error + 'static,
    {
        let svc = e.as_service_error();
        AwsFault {
            op,
            code: svc.and_then(|s| s.code().map(str::to_string)),
            message: svc.and_then(|s| s.message().map(str::to_string)),
            // NOT `.meta().request_id()` — that does not exist. `ErrorMetadata` exposes
            // `code`/`message`/`extra`; the `RequestId` trait lives in `aws-types`, which is
            // NOT a dependency of this crate. `extra("aws_request_id")` needs no new dep and
            // yields `None` when absent, which is the honest answer.
            request_id: svc.and_then(|s| s.meta().extra("aws_request_id").map(str::to_string)),
            http_status: match e {
                aws_sdk_dynamodb::error::SdkError::ServiceError(se) => {
                    Some(se.raw().status().as_u16())
                }
                aws_sdk_dynamodb::error::SdkError::ResponseError(re) => {
                    Some(re.raw().status().as_u16())
                }
                _ => None,
            },
            // A DELIBERATE APPROXIMATION, not "the SDK's own classification". Transport-level
            // timeouts and dispatch failures are retryable; a throttling *service* error is
            // retryable too and this does NOT catch it. Under-reporting is the safe direction —
            // a missing `[retryable]` costs a reader nothing, a false one sends them down a
            // wrong path at 3am.
            retryable: matches!(
                e,
                aws_sdk_dynamodb::error::SdkError::TimeoutError(_)
                    | aws_sdk_dynamodb::error::SdkError::DispatchFailure(_)
            ),
        }
    }

    /// A request we failed to BUILD — our own bug, not AWS's.
    ///
    /// Captures `Display`, deliberately, and NOT `Debug`. `BuildError`'s `Display` names the
    /// offending *field* (`"invalid field in input: {field}"`, `"{field} was missing"`) and
    /// never the value bound to it, so it is safe and genuinely diagnostic. Its `Debug` is not
    /// an audited surface and is therefore not adopted — the same rule as everywhere else in
    /// this module: capture what is bounded, never what merely happens to look small today.
    pub fn from_build_error(op: &'static str, e: &aws_sdk_dynamodb::error::BuildError) -> Self {
        AwsFault {
            op,
            code: Some("BuildError".to_string()),
            message: Some(e.to_string()),
            request_id: None,
            http_status: None,
            retryable: false,
        }
    }
}

impl std::fmt::Display for AwsFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed", self.op)?;
        if let Some(c) = &self.code {
            write!(f, " [{c}]")?;
        }
        if let Some(m) = &self.message {
            write!(f, ": {m}")?;
        }
        if let Some(s) = self.http_status {
            write!(f, " (http {s})")?;
        }
        if let Some(r) = &self.request_id {
            write!(f, " (request-id {r})")?;
        }
        if self.retryable {
            write!(f, " [retryable]")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_types::body::SdkBody;

    const SENTINEL: &str = "HB-GIFT-KEY-SENTINEL-0451";

    /// The exact payload the RECOMMENDED `ReturnValuesOnConditionCheckFailure::AllOld`
    /// line hands back on a failed conditional write of a claim carrying a revealed key.
    ///
    /// The `meta` is set deliberately. On the wire the SDK populates it while deserializing
    /// the error response; a fixture built straight from the builder has none, and `code()`
    /// then reads `None`. Setting it here keeps the fixture FAITHFUL to production rather
    /// than relaxing the assertion — what this test owns is that `AwsFault` reads the
    /// metadata, not that the SDK fills it in.
    fn ccf_sdk_error_with_item() -> aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::put_item::PutItemError,
        aws_smithy_runtime_api::http::Response,
    > {
        let ccf = aws_sdk_dynamodb::types::error::ConditionalCheckFailedException::builder()
            .item(
                "revealed_key",
                aws_sdk_dynamodb::types::AttributeValue::S(SENTINEL.to_string()),
            )
            .meta(
                aws_smithy_types::error::ErrorMetadata::builder()
                    .code("ConditionalCheckFailedException")
                    .message("The conditional request failed")
                    .build(),
            )
            .build();
        let op_err =
            aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(
                ccf,
            );
        let raw = aws_smithy_runtime_api::http::Response::new(
            400u16.try_into().unwrap(),
            SdkBody::empty(),
        );
        aws_sdk_dynamodb::error::SdkError::service_error(op_err, raw)
    }

    #[test]
    fn item_attributes_never_reach_the_fault() {
        let fault = AwsFault::from_sdk_error("put_item", &ccf_sdk_error_with_item());
        let rendered = format!("{fault}");
        assert!(
            !rendered.contains(SENTINEL),
            "item attribute reached AwsFault Display: {rendered}"
        );
        assert!(
            !format!("{fault:?}").contains(SENTINEL),
            "item attribute reached AwsFault Debug: {fault:?}"
        );
    }

    #[test]
    fn fault_still_says_which_call_and_what_aws_said() {
        let fault = AwsFault::from_sdk_error("put_item", &ccf_sdk_error_with_item());
        let rendered = format!("{fault}");
        assert!(
            rendered.contains("put_item"),
            "lost the operation: {rendered}"
        );
        assert!(
            rendered.contains("ConditionalCheckFailedException"),
            "lost the error code: {rendered}"
        );
    }

    #[test]
    fn http_status_is_captured() {
        let fault = AwsFault::from_sdk_error("put_item", &ccf_sdk_error_with_item());
        assert!(
            format!("{fault}").contains("http 400"),
            "lost the http status: {fault}"
        );
    }

    /// ALL FOUR unbounded arms must be exercised, not one.
    ///
    /// An earlier draft constructed only `construction_failure` under a plural name. Its
    /// rationale — that it catches a catch-all `_` capture — was true and INSUFFICIENT: it
    /// cannot catch a PER-ARM capture, and `from_sdk_error` hand-writes `ResponseError(re) =>`
    /// as its own arm, the one likeliest to grow one.
    /// *An arm that has never printed is a decoration.* (OMBB, step-5 gate.)
    #[test]
    fn opaque_arms_do_not_leak_their_payload() {
        const OPAQUE: &str = "OPAQUE-BOXERROR-PAYLOAD-9142";

        #[derive(Debug)]
        struct Nosy;
        impl std::fmt::Display for Nosy {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{OPAQUE}")
            }
        }
        impl std::error::Error for Nosy {}

        type E = aws_sdk_dynamodb::error::SdkError<
            aws_sdk_dynamodb::operation::put_item::PutItemError,
            aws_smithy_runtime_api::http::Response,
        >;

        fn raw() -> aws_smithy_runtime_api::http::Response {
            aws_smithy_runtime_api::http::Response::new(
                500u16.try_into().unwrap(),
                SdkBody::empty(),
            )
        }

        let cases: Vec<(&str, E)> = vec![
            ("ConstructionFailure", E::construction_failure(Nosy)),
            ("TimeoutError", E::timeout_error(Nosy)),
            (
                "DispatchFailure",
                E::dispatch_failure(aws_smithy_runtime_api::client::result::ConnectorError::io(
                    Box::new(Nosy),
                )),
            ),
            ("ResponseError", E::response_error(Nosy, raw())),
        ];

        for (name, e) in cases {
            let fault = AwsFault::from_sdk_error("put_item", &e);
            assert!(
                !format!("{fault}").contains(OPAQUE),
                "{name} payload reached the fault Display: {fault}"
            );
            assert!(
                !format!("{fault:?}").contains(OPAQUE),
                "{name} payload reached the fault Debug: {fault:?}"
            );
        }
    }
}
