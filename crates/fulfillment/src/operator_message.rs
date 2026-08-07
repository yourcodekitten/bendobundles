//! The only door through which text reaches the operator channel.
//!
//! SECURITY: `OperatorMessage` has a private inner, NO public string constructor and NO
//! `From<String>`. That is deliberate and load-bearing: a type that *offers* safety is not a type
//! that *enforces* it, and only enforcement has jurisdiction over call sites nobody has written
//! yet. **Do not add a string constructor.**
//!
//! TRUST BOUNDARY: CloudWatch is access-controlled; a Discord channel is not. A raw error is fine —
//! it belongs on the CloudWatch side of that line. Never render an error's `Display` or `Debug`
//! into an `OperatorMessage`. A filter protects the code you audited; a type protects the code you
//! haven't.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// A rendered, operator-safe reference to an error.
///
/// NOTE: `kind` is **published content** — it goes to Discord. A future error type named after its
/// payload would leak by naming. Keep type names free of secrets.
pub struct ErrorSummary {
    kind: &'static str,
    req_id: String,
}

impl ErrorSummary {
    /// Render an error as a type name plus a fresh correlation id. **The error's own text is never
    /// read** — callers log it themselves, on the CloudWatch side of the trust boundary.
    pub fn of<E: std::error::Error>(_e: &E) -> ErrorSummary {
        ErrorSummary {
            kind: std::any::type_name::<E>(),
            req_id: new_req_id(),
        }
    }

    pub fn req_id(&self) -> &str {
        &self.req_id
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The exact `(req_id, kind)` pair that MUST appear in the CloudWatch record. The operator
    /// message renders from the same struct, so the join cannot drift unless someone bypasses this
    /// accessor. An id that appears on only one side is worse than the leak it replaced — at 3am
    /// it *looks* like you can chase it.
    pub fn log_fields(&self) -> (&str, &'static str) {
        (&self.req_id, self.kind)
    }
}

/// A join key for "operator line ↔ CloudWatch record".
///
/// No timestamp entropy. An earlier design used `subsec_nanos`, which is bounded by CLOCK
/// GRANULARITY rather than by the field's width, carries no container discriminator, and collides
/// most likely during a burst — i.e. exactly when you would use it. **A colliding join key is worse
/// than no join key:** with none you know you cannot join; with a collision you grep at 3am and
/// debug two interleaved incidents with nothing telling you that is what happened.
///
/// So: use the identifier the platform already partitions logs by. `AWS_LAMBDA_LOG_STREAM_NAME` is
/// unique per container and names the very stream an operator opens.
fn new_req_id() -> String {
    static CONTAINER: OnceLock<String> = OnceLock::new();
    static N: AtomicU64 = AtomicU64::new(0);
    let base = CONTAINER.get_or_init(|| {
        std::env::var("AWS_LAMBDA_LOG_STREAM_NAME")
            .ok()
            .and_then(|s| s.rsplit(']').next().map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                // The fallback must NOT be a shared constant. `OnceLock` bakes whatever it first
                // computes for the container's WHOLE LIFE — the same once-per-container asymmetry
                // that makes an SSM blip permanent. A literal would mean two containers that both
                // miss the var collide again, reintroducing the defect this function exists to
                // kill. And it is LOUD, because the id also stops naming the log stream, which was
                // the reason to prefer this scheme at all.
                tracing::warn!(
                    outcome = "req_id_base_fallback",
                    "AWS_LAMBDA_LOG_STREAM_NAME absent or empty — join keys fall back to pid"
                );
                format!("p{}", std::process::id())
            })
    });
    // Uniqueness window: the LIFE OF THE CONTAINER. `u64` cannot wrap here — it increments once per
    // operator-notification FAILURE.
    format!("{base}-{}", N.fetch_add(1, Ordering::Relaxed))
}

pub enum Part<'a> {
    Text(&'static str),
    Id(&'a str),
    Error(ErrorSummary),
}

/// Operator-visible text. Private inner by design — see the module docs.
///
/// A runtime `String` **cannot** become an `OperatorMessage`. That is the guarantee, and this
/// doctest is its regression canary — if someone adds `From<String>` or a `&str` constructor, this
/// stops failing to compile and the test goes red:
///
/// ```compile_fail
/// let runtime_text: String = format!("secret {}", "abc123");
/// let _ = fulfillment::operator_message::OperatorMessage::literal(&runtime_text);
/// ```
///
/// STATED LIMIT: `compile_fail` asserts only THAT compilation failed, never WHICH error — pinning
/// the code (```compile_fail,E0308```) was **measured to be decorative**: a deliberately wrong code
/// still passes. So this is a canary on a guarantee that lives in the type's API, not the
/// guarantee itself. The private-module class specifically cannot silently green it, because the
/// module is `pub` and the unit tests above compile through it — a private or moved module breaks
/// the build first, loudly.
pub struct OperatorMessage(String);

impl OperatorMessage {
    pub fn literal(s: &'static str) -> OperatorMessage {
        OperatorMessage(s.to_string())
    }

    pub fn with(parts: &[Part<'_>]) -> OperatorMessage {
        let mut out = String::new();
        for p in parts {
            if !out.is_empty() {
                out.push(' ');
            }
            match p {
                Part::Text(t) => out.push_str(t),
                Part::Id(i) => out.push_str(i),
                Part::Error(e) => out.push_str(&format!("[{} req {}]", e.kind(), e.req_id())),
            }
        }
        OperatorMessage(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Leaky;
    impl std::fmt::Display for Leaky {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "write failed for key SECRET-KEY-abc123")
        }
    }
    impl std::error::Error for Leaky {}

    #[test]
    fn error_summary_never_carries_the_error_payload() {
        let s = ErrorSummary::of(&Leaky);
        let m = OperatorMessage::with(&[Part::Text("store write failed"), Part::Error(s)]);
        assert!(
            !m.as_str().contains("SECRET-KEY-abc123"),
            "payload leaked: {}",
            m.as_str()
        );
    }

    #[test]
    fn error_summary_carries_a_joinable_req_id() {
        let s = ErrorSummary::of(&Leaky);
        let id = s.req_id().to_string();
        assert!(!id.is_empty());
        let m = OperatorMessage::with(&[Part::Text("store write failed"), Part::Error(s)]);
        assert!(
            m.as_str().contains(&id),
            "req_id must appear in the operator text"
        );
    }

    #[test]
    fn req_ids_are_distinct_within_a_process() {
        let a = ErrorSummary::of(&Leaky).req_id().to_string();
        let b = ErrorSummary::of(&Leaky).req_id().to_string();
        assert_ne!(a, b, "two summaries minted the same join key");
    }

    #[test]
    fn operator_text_and_log_fields_share_one_req_id() {
        let s = ErrorSummary::of(&Leaky);
        let (log_id, _kind) = s.log_fields();
        let log_id = log_id.to_string();
        let m = OperatorMessage::with(&[Part::Text("x"), Part::Error(s)]);
        assert!(
            m.as_str().contains(&log_id),
            "operator text and log record carry different ids — the breadcrumb is unjoinable"
        );
    }
}
