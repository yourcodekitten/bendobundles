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
        // REDUNDANT #1 of 3 — see the map below before deleting this `.trim()`.
        let body = self.0.trim();
        // THE EMPTY-CHUNK PROPERTY IS HELD BY THREE MECHANISMS JOINTLY, and the SITES ARE LABELLED
        // `REDUNDANT #n of 3` so the word reaches you at the moment you are deciding to delete one.
        // An instruction here only helps a reader who has already scrolled to here; the labels do
        // not rely on anyone remembering anything. (Lilith's point, adopted — it is strictly better
        // than the paragraph it sits in.)
        //   1. `self.0.trim()` above          2. `!cur.trim().is_empty()` at the tail push
        //   3. `.filter(|p| !p.trim().is_empty())` before labelling
        //
        // MEASURED, three cells, not assumed:
        //   remove any ONE                    → green (46/46 then, 50/50 now)
        //   remove #1 AND #2, keep #3         → GREEN — the filter alone holds the property
        //   remove ALL THREE                  → RED: `got chunks for "   "`
        // The middle cell is the one that decides whether "change one at a time" is a real
        // discipline or a decorative one. It is real: since the all-three state is RED, whatever
        // order you delete in, the THIRD deletion turns the suite red — so a one-at-a-time deleter
        // is always caught before shipping, and a batch deleter never is.
        // THE HAZARD IS THE SEQUENCE, NOT ANY STEP: each deletion is individually justified by a
        // green run, every one of those runs is honest, and the composition reintroduces the bug.
        // Single-removal testing is structurally blind to redundancy — "removed it, still green,
        // therefore dead" is invalid anywhere a second provider might exist.
        //
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
                        if !cur.matches("**").count().is_multiple_of(2) {
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
        // REDUNDANT #2 of 3 (siblings: `self.0.trim()` at the top, the empty-part filter below).
        // Deleting this alone is green and that green result is honest — see the map at the top.
        if !cur.trim().is_empty() {
            parts.push(cur);
        }

        // Filter FIRST, then count, then label. (This filter's real job is a whitespace-only
        // INTERMEDIATE part, not the wholly-empty input above — see the fast-path note.) Computing `n` before filtering left gaps in the
        // labels — "(1/3)" and "(3/3)" with no "(2/3)" — which reads to an operator as a LOST
        // MESSAGE. The no-empty rule and the labelling rule are coupled.
        // REDUNDANT #3 of 3 (siblings: `self.0.trim()` at the top, `!cur.trim().is_empty()` above).
        // MEASURED to be the strongest of the three: with #1 and #2 both removed, this one alone
        // keeps `empty_input_yields_no_chunks_at_all` green. It is also the only one of the three
        // with a SECOND job (the label-gap fix, above) — so it is the last one you should reach
        // for, not the first.
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
            out.push_str(&render(p));
        }
        OperatorMessage(out)
    }

    /// Substitute `args` into `template`'s `{}` placeholders, in order.
    ///
    /// WHY THIS EXISTS (and why it is not a weakening of the module's guarantee): `with` joins its
    /// parts with a space, which is fine for `text id [err]` shapes and wrong for the ~40 real call
    /// sites that interpolate a value MID-SENTENCE — `with` renders those as
    /// `claim abc ( xyz ) hit a DEAD key`. Degrading every operator message is not an acceptable
    /// price for the trust boundary, so the boundary is held a different way here.
    ///
    /// **BOTH doors stay shut, by TYPE, exactly as they are for `literal` and `with`:**
    ///   - `template` is `&'static str` — it cannot carry a runtime value, so no `format!` result
    ///     can enter through it.
    ///   - `args` are `Part` — there is no `Part` that renders an error's `Display`/`Debug`, so an
    ///     error can only appear as `Part::Error`, i.e. as `[kind req id]`.
    ///
    /// There is still **no public `String` constructor**; the `compile_fail` canary above is
    /// unaffected.
    ///
    /// ARITY IS CHECKED AT RUNTIME, LOUDLY — `format!` checks it at compile time and this cannot,
    /// which is a real capability lost. A silent mismatch (message renders with a value missing,
    /// nobody notices) is precisely the defect class this module exists to kill, so a mismatch is
    /// made VISIBLE in the operator text AND recorded out of band. Pinned by
    /// `fmt_arity_mismatch_is_visible_not_silent`.
    pub fn fmt(template: &'static str, args: &[Part<'_>]) -> OperatorMessage {
        let mut out = String::new();
        let mut rest = template;
        let mut used = 0usize;
        while let Some(i) = rest.find("{}") {
            out.push_str(&rest[..i]);
            match args.get(used) {
                Some(p) => out.push_str(&render(p)),
                // Fewer args than placeholders. Never leave a bare `{}` — an operator reading
                // `claim {} is stuck` cannot tell a template bug from a genuinely absent id.
                None => out.push_str("{?}"),
            }
            used += 1;
            rest = &rest[i + 2..];
        }
        out.push_str(rest);
        if used != args.len() {
            // Out of band on purpose: the operator channel is the thing that just misrendered, so
            // it is the one place this cannot be reported. Same rule as a failed send.
            tracing::error!(
                outcome = "operator_template_arity",
                placeholders = used,
                args = args.len(),
                "operator message template and argument count disagree — message misrendered"
            );
            if used < args.len() {
                // Extra args are otherwise INVISIBLE — the message just quietly loses a value.
                out.push_str(" [!template dropped ");
                out.push_str(&(args.len() - used).to_string());
                out.push_str(" value(s)]");
            }
        }
        OperatorMessage(out)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The single rendering of a `Part`. Both `with` and `fmt` go through here so the two constructors
/// cannot disagree about what an error looks like on the operator side.
fn render(p: &Part<'_>) -> String {
    match p {
        Part::Text(t) => (*t).to_string(),
        Part::Id(i) => (*i).to_string(),
        Part::Error(e) => format!("[{} req {}]", e.kind(), e.req_id()),
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
            assert!(
                c.chars().count() <= 2000,
                "chunk too long: {}",
                c.chars().count()
            );
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
            assert!(
                c.chars().count() <= 2000,
                "bound violated: {}",
                c.chars().count()
            );
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

    // ---------------------------------------------------------------------------------------
    // `fmt` — the mid-sentence constructor. Added during Task 6 because the call sites were 53,
    // not the ~20 the plan assumed, and ~40 of them interpolate a value mid-prose where `with`'s
    // space-join renders `claim abc ( xyz ) hit a DEAD key`.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn fmt_substitutes_in_order_and_keeps_punctuation() {
        let m = OperatorMessage::fmt(
            "claim {} ({}) hit a DEAD key — humble says: \"{}\".",
            &[Part::Id("c-1"), Part::Id("g-2"), Part::Id("revoked")],
        );
        // The whole reason `fmt` exists: `with` would render `claim c-1 ( g-2 ) hit a DEAD key`.
        assert_eq!(
            m.as_str(),
            "claim c-1 (g-2) hit a DEAD key — humble says: \"revoked\"."
        );
    }

    #[test]
    fn fmt_renders_an_error_through_the_same_door_as_with() {
        // `render` is shared, so the two constructors cannot disagree about what an error looks
        // like on the operator side — the property that makes ONE audit of `render` sufficient.
        let s = ErrorSummary::of(&Leaky);
        let (req, kind) = s.log_fields();
        let (req, kind) = (req.to_string(), kind);
        let m = OperatorMessage::fmt("write failed: {}", &[Part::Error(s)]);
        assert_eq!(m.as_str(), format!("write failed: [{kind} req {req}]"));
        assert!(
            !m.as_str().contains("SECRET-KEY-abc123"),
            "the error's own text reached the operator channel"
        );
    }

    #[test]
    fn fmt_arity_mismatch_is_visible_not_silent() {
        // TOO FEW ARGS: never leave a bare `{}` — an operator cannot tell a template bug from a
        // genuinely absent id.
        let short = OperatorMessage::fmt("claim {} ({}) is stuck", &[Part::Id("c-1")]);
        assert_eq!(short.as_str(), "claim c-1 ({?}) is stuck");

        // TOO MANY ARGS is the nastier one: without the trailing note the message is perfectly
        // well-formed prose that has quietly LOST a value, which is exactly this arc's defect.
        let long = OperatorMessage::fmt("claim {} is stuck", &[Part::Id("c-1"), Part::Id("g-2")]);
        assert!(
            long.as_str().contains("dropped 1 value(s)"),
            "a dropped argument left no trace in the message: {}",
            long.as_str()
        );
    }

    #[test]
    fn fmt_with_no_placeholders_and_no_args_is_the_template() {
        let m = OperatorMessage::fmt("nothing to substitute", &[]);
        assert_eq!(m.as_str(), "nothing to substitute");
    }
}
