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


/// Discord's hard limit on `content`.
const DISCORD_MAX: usize = 2000;
/// Room reserved for the ` (12/34)` label. 10 fits ` (999/999)` exactly.
const LABEL_RESERVE: usize = 10;
/// Room reserved for the forced-cut marker ` …`, RESERVED not appended — appending it after would
/// push a maximally-packed chunk over the bound BECAUSE of the marker added to respect the bound.
const MARKER_RESERVE: usize = 2;

impl OperatorMessage {
    /// Split into Discord-postable bodies, each ≤2000 chars INCLUDING `prefix` and the label.
    ///
    /// PRECEDENCE — three requirements interact here, not two:
    ///   1. the 2000 bound ALWAYS wins (it is the one with a real 400 behind it)
    ///   2. then "no empty chunk" (the only end of this axis with observed production failures —
    ///      an empty `content` is a Discord 400, and this seam was immune via its unconditional
    ///      prefix until chunking was added, so the guard exists BECAUSE of this change)
    ///   3. then "no mid-token split" (cosmetic — but a boundary that merely LOOKS like damage
    ///      costs the same investigation as real damage)
    ///
    /// A forced mid-token cut carries ` …` so it is visible rather than silent.
    pub fn chunks(&self, prefix: &str) -> Vec<String> {
        let body = self.0.trim();
        // NOTE (deliberately no early-return guard here): an earlier version had
        // `if body.is_empty() { return Vec::new(); }`. It was DEAD CODE and is deleted rather
        // than re-labelled. `body.is_empty()` implies `split_inclusive('\n')` yields zero items,
        // so the loop never runs, `cur` stays empty, nothing is pushed, and the result is empty
        // anyway — there is no input for which the guard changes behaviour. The empty-chunk
        // property is STRUCTURAL, and `empty_input_yields_no_chunks_at_all` pins it (verified it
        // can fail: breaking `chunks` outright turns it red).
        let budget = DISCORD_MAX
            .saturating_sub(prefix.chars().count())
            .saturating_sub(LABEL_RESERVE)
            .saturating_sub(MARKER_RESERVE);

        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        for line in body.split_inclusive('\n') {
            if !cur.is_empty() && cur.chars().count() + line.chars().count() > budget {
                parts.push(std::mem::take(&mut cur));
            }
            if line.chars().count() > budget {
                for ch in line.chars() {
                    if cur.chars().count() + 1 > budget {
                        // Bound wins. Mark a forced mid-token cut so it is never silent.
                        if cur.matches("**").count() % 2 != 0 {
                            cur.push_str(" …");
                        }
                        parts.push(std::mem::take(&mut cur));
                    }
                    cur.push(ch);
                }
            } else {
                cur.push_str(line);
            }
        }
        if !cur.trim().is_empty() {
            parts.push(cur);
        }

        // Filter FIRST, then count, then label. (This filter's real job is a whitespace-only
        // INTERMEDIATE part, not the wholly-empty input above — see the fast-path note.) Computing `n` before filtering left gaps in the
        // labels — "(1/3)" and "(3/3)" with no "(2/3)" — which reads to an operator as a LOST
        // MESSAGE. The no-empty rule and the labelling rule are coupled.
        let parts: Vec<String> = parts.into_iter().filter(|p| !p.trim().is_empty()).collect();
        let n = parts.len();
        parts
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                if n > 1 {
                    format!("{prefix}{} ({}/{n})", p.trim_end(), i + 1)
                } else {
                    format!("{prefix}{}", p.trim_end())
                }
            })
            .collect()
    }
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


    const PREFIX: &str = "🐱 bendobundles: ";

    // GATE FIX B3: an earlier version iterated `for c in m.chunks(..)` on EMPTY input. `chunks()`
    // returns an empty Vec there, so the loop body never ran and the test passed regardless of the
    // implementation — vacuous, and it was guarding the ONE requirement with real production
    // evidence behind it (an empty Discord `content` is a 400). Split into two that can each fail.
    #[test]
    fn empty_input_yields_no_chunks_at_all() {
        for input in ["", "   ", "\n\n"] {
            let m = OperatorMessage::literal(Box::leak(input.to_string().into_boxed_str()));
            assert!(
                m.chunks(PREFIX).is_empty(),
                "empty input must post nothing; got chunks for {input:?}"
            );
        }
    }

    #[test]
    fn no_emitted_chunk_is_empty_or_prefix_only() {
        let m = OperatorMessage::literal(Box::leak("y".repeat(5000).into_boxed_str()));
        let cs = m.chunks(PREFIX);
        assert!(!cs.is_empty());
        for c in &cs {
            assert!(c.len() > PREFIX.len(), "prefix-only chunk: {c:?}");
        }
    }

    #[test]
    fn chunks_are_bounded_including_prefix_and_label() {
        let m = OperatorMessage::literal(Box::leak("x".repeat(5000).into_boxed_str()));
        for c in m.chunks(PREFIX) {
            assert!(c.chars().count() <= 2000, "chunk too long: {}", c.chars().count());
        }
    }

    #[test]
    fn multi_chunk_messages_are_labelled() {
        let m = OperatorMessage::literal(Box::leak("z".repeat(5000).into_boxed_str()));
        let cs = m.chunks(PREFIX);
        assert!(cs.len() > 1);
        assert!(cs[0].contains(&format!("(1/{})", cs.len())));
    }

    // GATE FIX M1: the 2000 bound and "never split mid-token" CONFLICT. Precedence is explicit —
    // THE BOUND ALWAYS WINS, because exceeding it is what actually produces a Discord 400.
    #[test]
    fn the_2000_bound_wins_over_token_integrity() {
        let body = format!("{}**{}", "a".repeat(1999), "b".repeat(1999)); // ONE line, unbalanced **
        let m = OperatorMessage::literal(Box::leak(body.into_boxed_str()));
        for c in m.chunks(PREFIX) {
            assert!(c.chars().count() <= 2000, "bound violated: {}", c.chars().count());
        }
    }

    // GATE FIX M2: an earlier version asserted `stars % 2 == 0` UNCONDITIONALLY, contradicting the
    // stated precedence — a forced cut deliberately emits an odd-star chunk carrying the marker.
    // It passed only because the fixture never forced a cut: passing by input luck.
    #[test]
    fn chunks_are_balanced_or_carry_the_forced_cut_marker() {
        let body = format!("{}\n**bold**\n{}", "a".repeat(1900), "b".repeat(1900));
        let m = OperatorMessage::literal(Box::leak(body.into_boxed_str()));
        for c in m.chunks(PREFIX) {
            let balanced = c.matches("**").count() % 2 == 0;
            assert!(
                balanced || c.contains(" …"),
                "chunk ends inside a ** pair with no forced-cut marker: {c}"
            );
        }
    }

    #[test]
    fn chunk_bodies_concatenate_back_to_the_input() {
        let input = "q".repeat(5000);
        let m = OperatorMessage::literal(Box::leak(input.clone().into_boxed_str()));
        let joined: String = m
            .chunks(PREFIX)
            .iter()
            .map(|c| {
                let body = c.strip_prefix(PREFIX).unwrap_or(c);
                let body = match body.rfind(" (") {
                    Some(i) if body.ends_with(')') => &body[..i],
                    _ => body,
                };
                body.to_string()
            })
            .collect();
        assert_eq!(joined, input, "chunking lost or altered content");
    }

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
