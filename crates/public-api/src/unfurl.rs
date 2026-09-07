//! The wrapping paper: per-token og meta swapped into the deployed index.html.
//! Spec: docs/spec-wrapping-paper.md (r2). The card is the wrapped box, never
//! the contents; personalized ONLY for active|sealed; audience = the room.
use async_trait::async_trait;
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use domain::og_text;
use std::sync::Arc;
use time::OffsetDateTime;

use crate::AppState;

pub(crate) const OG_BEGIN: &str = "<!-- og:begin -->";
pub(crate) const OG_END: &str = "<!-- og:end -->";

#[derive(Debug)]
pub(crate) struct MarkerAbsent;

pub(crate) fn swap_og_block(template: &str, meta_html: &str) -> Result<String, MarkerAbsent> {
    let start = template.find(OG_BEGIN).ok_or(MarkerAbsent)?;
    let end_rel = template[start..].find(OG_END).ok_or(MarkerAbsent)?;
    let end = start + end_rel + OG_END.len();
    let mut out = String::with_capacity(template.len() + meta_html.len());
    out.push_str(&template[..start]);
    out.push_str(OG_BEGIN);
    out.push('\n');
    out.push_str(meta_html);
    out.push('\n');
    out.push_str(OG_END);
    out.push_str(&template[end..]);
    Ok(out)
}

const WRAPS: [&str; 8] = [
    "clay", "rust", "mustard", "moss", "pine", "slate", "heather", "mauve",
];

pub(crate) fn wrap_variant(token: &str) -> &'static str {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    WRAPS[(h % 8) as usize]
}

fn count_words(n: usize) -> String {
    const W: [&str; 12] = [
        "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven",
        "twelve",
    ];
    if (1..=12).contains(&n) {
        W[n - 1].to_string()
    } else {
        "a dozen and more".to_string()
    }
}

fn meta_block(title: &str, desc: &str, image: &str, alt: &str) -> String {
    // INVARIANT (tested, not asserted): every interpolated value has passed
    // og_text — including image URLs (escaping a URL is harmless and correct in
    // attribute context; &→&amp;) — so a future base_url-from-Host refactor
    // cannot bypass the escaper silently (Lilith's MAJOR-1). `alt` is run through
    // og_text HERE (not by callers) so the invariant holds regardless of what a
    // future caller passes — both call sites are literals today, so this is a
    // no-op on current output (MINOR-1, 2026-09-07 final review).
    let alt = og_text(alt, 200);
    format!(
        "<meta property=\"og:type\" content=\"website\" />\n\
         <meta property=\"og:site_name\" content=\"bendobundles\" />\n\
         <meta property=\"og:title\" content=\"{title}\" />\n\
         <meta property=\"og:description\" content=\"{desc}\" />\n\
         <meta property=\"og:image\" content=\"{image}\" />\n\
         <meta property=\"og:image:width\" content=\"1200\" />\n\
         <meta property=\"og:image:height\" content=\"630\" />\n\
         <meta property=\"og:image:alt\" content=\"{alt}\" />\n\
         <meta name=\"twitter:card\" content=\"summary_large_image\" />\n\
         <meta name=\"twitter:title\" content=\"{title}\" />\n\
         <meta name=\"twitter:description\" content=\"{desc}\" />\n\
         <meta name=\"twitter:image\" content=\"{image}\" />"
    )
}

pub(crate) fn meta_for_link(
    link: &domain::Link,
    now: time::OffsetDateTime,
    base_url: &str,
) -> Option<String> {
    let sealed = match link.can_claim(now) {
        Ok(()) => false,
        Err(domain::ClaimRefusal::Sealed) => true,
        Err(_) => return None, // revoked|expired|exhausted → generic (spec D5)
    };
    let label = og_text(&link.label, 80);
    let title = format!("🎁 ben wrapped something for {label} ♡");
    let desc = if sealed {
        "sealed for now. good things wait.".to_string()
    } else {
        match &link.curated_game_ids {
            Some(ids) if !ids.is_empty() => {
                let n = ids.len();
                let s = if n == 1 { "" } else { "s" };
                format!(
                    "{} treasure{s} inside, chosen for you. tap to unwrap.",
                    count_words(n)
                )
            }
            // Open-shelf link (curated_game_ids: None or empty) — NOT curated,
            // so this must not borrow the curated row's "chosen for you" claim
            // (pass-1 product review, MAJOR: 18/18 production links are this
            // shape as of docs/spec-attic-whispers.md's census).
            _ => "ben opened his stash for you. tap to look inside.".to_string(),
        }
    };
    let image = og_text(
        &format!("{base_url}/art/wrap-{}.png", wrap_variant(&link.token)),
        200,
    );
    Some(meta_block(
        &title,
        &og_text(&desc, 200),
        &image,
        "a tiny pixel adventurer walking toward a wrapped present, drawn in shades of pea green",
    ))
}

pub(crate) fn meta_for_shelf(friend: &domain::Friend, base_url: &str) -> String {
    let name = og_text(&friend.name, 80);
    meta_block(
        &format!("📚 the shelf ben keeps for {name} ♡"),
        "every game he's given you, all in one warm place.",
        &og_text(&format!("{base_url}/art/wrap-shelf.png"), 200),
        "a tiny pixel adventurer walking toward a shelf of games, drawn in shades of pea green",
    )
}

// ── TemplateSource: where the deployed index.html comes from ──────────────────

/// Fetch failure — the ONLY error shape this arc distinguishes (spec: template
/// source down or unreadable ⇒ 500, never a partial/garbled card).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateError {
    Unavailable,
}

/// Where the lambda gets the deployed `index.html` to swap the og block into.
/// `#[async_trait]` mirrors this crate's existing idiom (`Invoker`, lib.rs:27) —
/// no new dependency shape for the crate to carry.
#[async_trait]
pub trait TemplateSource: Send + Sync {
    async fn fetch(&self) -> Result<String, TemplateError>;
}

/// Production `TemplateSource`: the web S3 bucket's `index.html`, with a 60s
/// in-memory cache so a burst of unfurls (a link pasted into a busy channel)
/// costs one `GetObject`, not one per request (spec D3).
pub struct S3Template {
    client: aws_sdk_s3::Client,
    bucket: String,
    cache: tokio::sync::RwLock<Option<(std::time::Instant, String)>>,
}

impl S3Template {
    const TTL: std::time::Duration = std::time::Duration::from_secs(60);

    pub fn new(client: aws_sdk_s3::Client, bucket: String) -> Self {
        Self {
            client,
            bucket,
            cache: tokio::sync::RwLock::new(None),
        }
    }

    async fn fetch_inner(&self) -> Result<String, TemplateError> {
        if let Some((at, s)) = self.cache.read().await.as_ref()
            && at.elapsed() < Self::TTL
        {
            return Ok(s.clone());
        }
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key("index.html")
            .send()
            .await
            .map_err(|_| TemplateError::Unavailable)?;
        let bytes = out
            .body
            .collect()
            .await
            .map_err(|_| TemplateError::Unavailable)?;
        let s = String::from_utf8(bytes.into_bytes().to_vec())
            .map_err(|_| TemplateError::Unavailable)?;
        *self.cache.write().await = Some((std::time::Instant::now(), s.clone()));
        Ok(s)
    }
}

#[async_trait]
impl TemplateSource for S3Template {
    async fn fetch(&self) -> Result<String, TemplateError> {
        self.fetch_inner().await
    }
}

// ── EMF witness (spec D3: marker absence must announce itself) ────────────────

/// CloudWatch Embedded Metric Format blob for the marker-absent witness. Built
/// as a value (not a raw string) so the shape is unit-testable independent of
/// how it's printed.
fn emf_marker_absent_blob() -> serde_json::Value {
    serde_json::json!({
        "_aws": {
            "Timestamp": OffsetDateTime::now_utc().unix_timestamp() * 1000,
            "CloudWatchMetrics": [{
                "Namespace": "bendobundles/unfurl",
                "Dimensions": [[]],
                "Metrics": [{"Name": "UnfurlMarkerAbsent", "Unit": "Count"}]
            }]
        },
        "UnfurlMarkerAbsent": 1
    })
}

/// One EMF line to stdout → CloudWatch auto-extracts the metric. Called only on
/// the degrade path (marker absent) — this is a witness, not a request-rate metric.
fn emit_marker_absent_metric() {
    println!("{}", emf_marker_absent_blob());
}

// ── Response helpers ────────────────────────────────────────────────────────

/// 200, the given HTML body, with the headers every unfurl response carries
/// (personalized OR generic — spec: both are cacheable, both are `text/html`).
fn html_response(body: String) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=60"),
        ],
        body,
    )
        .into_response()
}

fn template_unavailable_response() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": "try again"})),
    )
        .into_response()
}

/// Fetch the deployed template, or the 500 response this arc serves when there
/// is no source configured or the source errored (spec: "template source None
/// or fetch error → 500 JSON").
async fn fetch_template(template: &Option<Arc<dyn TemplateSource>>) -> Result<String, Response> {
    match template {
        None => Err(template_unavailable_response()),
        Some(t) => match t.fetch().await {
            Ok(s) => Ok(s),
            Err(TemplateError::Unavailable) => Err(template_unavailable_response()),
        },
    }
}

// ── GET /l/{token} — personalized gift unfurl ──────────────────────────────────

pub(crate) async fn handle_unfurl_link(
    State(s): State<AppState>,
    Path(token): Path<String>,
) -> Response {
    let template = match fetch_template(&s.template).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let link = match s.store.get_link(&token).await {
        Ok(Some(l)) => l,
        // Unknown token: byte-identical to a dead link (spec D5 — no oracle,
        // no error card ever broadcast to a chat channel).
        Ok(None) => return html_response(template),
        Err(_) => return template_unavailable_response(),
    };

    let now = OffsetDateTime::now_utc();
    let Some(meta_html) = meta_for_link(&link, now, &s.base_url) else {
        // Dead state (revoked/expired/exhausted) — serve the generic card, not an error.
        return html_response(template);
    };

    match swap_og_block(&template, &meta_html) {
        Ok(swapped) => html_response(swapped),
        Err(MarkerAbsent) => {
            // MINOR-2 (2026-09-07 final review): marker absence is a DEPLOY
            // property (the template is missing markers), not a per-token one —
            // the witness needs no token at all. A capability token doesn't
            // belong in logs at full length, so only a short, non-reconstructible
            // prefix is logged, purely to help correlate repeated hits.
            tracing::error!(
                token_prefix = format!("{}…", &token[..8.min(token.len())]),
                "unfurl: deployed template is missing the og markers"
            );
            emit_marker_absent_metric();
            html_response(template)
        }
    }
}

// ── GET /s/{token} — shelf unfurl ──────────────────────────────────────────────

pub(crate) async fn handle_unfurl_shelf(
    State(s): State<AppState>,
    Path(token): Path<String>,
) -> Response {
    let template = match fetch_template(&s.template).await {
        Ok(t) => t,
        Err(resp) => return resp,
    };

    let friend = match s.store.get_friend_by_shelf_token(&token).await {
        Ok(Some(f)) => f,
        Ok(None) => return html_response(template),
        Err(_) => return template_unavailable_response(),
    };

    let meta_html = meta_for_shelf(&friend, &s.base_url);
    match swap_og_block(&template, &meta_html) {
        Ok(swapped) => html_response(swapped),
        Err(MarkerAbsent) => {
            // MINOR-2 (2026-09-07 final review): same rationale as the link
            // handler above — marker absence is a deploy property, no full
            // token needed, only a short prefix for correlation.
            tracing::error!(
                token_prefix = format!("{}…", &token[..8.min(token.len())]),
                "unfurl: deployed template is missing the og markers (shelf)"
            );
            emit_marker_absent_metric();
            html_response(template)
        }
    }
}

#[cfg(test)]
mod tests {
    use time::ext::NumericalDuration;

    /// The EMF witness's shape: parses, and carries the exact namespace + metric
    /// name + value the marker-absent degrade path is supposed to announce
    /// (spec D3). The handler-level tests (unfurl_test.rs) prove the DEGRADE
    /// half (generic card served); this proves the WITNESS half.
    #[test]
    fn emf_blob_for_marker_absent_has_correct_shape() {
        let v = super::emf_marker_absent_blob();
        assert_eq!(
            v["_aws"]["CloudWatchMetrics"][0]["Namespace"],
            "bendobundles/unfurl"
        );
        assert_eq!(
            v["_aws"]["CloudWatchMetrics"][0]["Metrics"][0]["Name"],
            "UnfurlMarkerAbsent"
        );
        assert_eq!(v["UnfurlMarkerAbsent"], 1);
        // Round-trips through the exact printed form (println!("{}", v)) — a
        // shape check that only holds on the Value would miss a Display bug.
        let printed = v.to_string();
        let reparsed: serde_json::Value =
            serde_json::from_str(&printed).expect("EMF line must be valid JSON");
        assert_eq!(reparsed, v);
    }

    fn test_link() -> domain::Link {
        domain::Link {
            token: "abc123".into(),
            label: "dave".into(),
            gift_note: None,
            thank_note: None,
            thanked_at: None,
            claims_allowed: 1,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            unlock_at: None,
            curated_game_ids: None,
            friend_id: None,
            created_at: time::macros::datetime!(2026-07-02 00:00 UTC),
        }
    }

    #[test]
    fn wrap_variant_is_pinned_forever() {
        // GOLDEN: literal pairs — the paper is a promise (spec D1). A determinism-only
        // or spread-only assertion lets a hash-constant refactor slip through green;
        // only independently-computed literals pin the promise.
        // Pins computed INDEPENDENTLY of the implementation (two seats agreeing
        // digit-for-digit) — never print-then-pin, which goldens the first bug.
        // And never REPEATED-UNIT tokens: FNV-1a's step is a bijection on Z/8 and
        // the offset basis &7 = 5, so a unit repeated >=4 times lands on WRAPS[5]
        // "slate" every time (measured; reps=2 is mixed ~60/40) — three such pins
        // are one assertion in a trenchcoat. The exact law (2000/2000 trials): a
        // repeated unit lands on slate iff the ORDER of its per-period permutation
        // divides the repeat count — observed orders {1,2,4}, so any reps
        // divisible by 4 returns to the offset basis; ">=4" coincides only because
        // 64 is a power of two. Rule: COMPUTE the bucket for a candidate pin,
        // never classify the token (Lilith's blocker + law, OMBB's quantifier,
        // both verified on this seat).
        assert_eq!(
            super::wrap_variant("8af17e0500caf83ec1172baf05a661ba1ee4ab02114f643b4ab5d2efe9ed80ec"),
            "clay"
        );
        assert_eq!(
            super::wrap_variant("b8b7c23ab0e0c45567869d4c98c9d489c8000a21da777c54e4cdeba61a513957"),
            "rust"
        );
        assert_eq!(
            super::wrap_variant("a0a8bddef089f638f98ca3f13aead6a96aaf25955eaeaa1a614c9a38427e6092"),
            "mustard"
        );
        let all: std::collections::HashSet<_> = (0..64)
            .map(|i| super::wrap_variant(&format!("{i:064x}")))
            .collect();
        assert!(
            all.len() >= 6,
            "64 tokens should hit most of 8 buckets: got {}",
            all.len()
        );
    }

    #[test]
    fn swap_og_block_replaces_between_markers() {
        let t = "head\n<!-- og:begin -->\nOLD\n<!-- og:end -->\ntail";
        let out = super::swap_og_block(t, "NEW").unwrap();
        assert!(out.contains("NEW") && !out.contains("OLD"));
        assert!(out.starts_with("head\n") && out.ends_with("\ntail"));
    }

    #[test]
    fn swap_og_block_absent_marker_is_typed() {
        assert!(super::swap_og_block("no markers here", "X").is_err());
    }

    #[test]
    fn meta_for_link_active_curated_counts_in_words() {
        let mut link = test_link(); // build with the same helper style store tests use
        link.curated_game_ids = Some(vec!["a".into(), "b".into(), "c".into()]);
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example")
            .unwrap();
        assert!(m.contains("three treasures inside, chosen for you. tap to unwrap."));
        assert!(m.contains("ben wrapped something for"));
        assert!(m.contains("/art/wrap-")); // one of the 8
    }

    #[test]
    fn meta_for_link_active_uncurated_is_open_shelf_not_chosen_for_you() {
        // pass-1 product review MAJOR: curated_game_ids: None means open shelf
        // (whole catalog, nothing hand-picked) — the copy must not assert
        // curation that didn't happen.
        let link = test_link(); // curated_game_ids: None, from the shared helper
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example")
            .unwrap();
        assert!(m.contains("ben opened his stash for you. tap to look inside."));
        assert!(
            !m.contains("chosen for you"),
            "open-shelf card must not claim curation"
        );
    }

    #[test]
    fn meta_for_link_active_empty_curated_list_is_also_open_shelf() {
        // Some(vec![]) must hit the same open-shelf arm as None — both mean
        // "nothing was actually chosen."
        let mut link = test_link();
        link.curated_game_ids = Some(vec![]);
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example")
            .unwrap();
        assert!(m.contains("ben opened his stash for you. tap to look inside."));
    }

    #[test]
    fn meta_for_link_sealed_has_state_but_never_a_clock() {
        let mut link = test_link();
        link.unlock_at = Some(time::OffsetDateTime::now_utc() + 2.days());
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example")
            .unwrap();
        assert!(m.contains("sealed for now. good things wait."));
        let year = time::OffsetDateTime::now_utc().year().to_string();
        assert!(
            !m.contains(&year),
            "no date-ish content in a broadcast card"
        );
    }

    #[test]
    fn meta_for_link_dead_states_are_none() {
        let mut link = test_link();
        link.revoked = true;
        assert!(
            super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example")
                .is_none()
        );
    }

    #[test]
    fn hostile_base_url_cannot_change_structure_either() {
        // base_url is server config today (router()'s 4th param) — this arm exists
        // for the future refactor that derives it from a request.
        let link = test_link();
        let m = super::meta_for_link(
            &link,
            time::OffsetDateTime::now_utc(),
            r#"https://x"><script>"#,
        )
        .unwrap();
        for line in m.lines().filter(|l| !l.trim().is_empty()) {
            assert_eq!(line.trim().matches('<').count(), 1, "injected < in: {line}");
        }
    }

    #[test]
    fn hostile_label_cannot_change_structure_at_the_meta_layer() {
        let mut link = test_link();
        link.label = r#"" onload=x><script>"#.into();
        let m = super::meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example")
            .unwrap();
        // No raw < > outside tag boundaries: every line parses as a <meta …/> element.
        for line in m.lines().filter(|l| !l.trim().is_empty()) {
            let l = line.trim();
            assert!(
                l.starts_with("<meta ") && l.ends_with("/>"),
                "unexpected line: {l}"
            );
            assert_eq!(l.matches('<').count(), 1, "injected < in: {l}");
        }
    }
}
