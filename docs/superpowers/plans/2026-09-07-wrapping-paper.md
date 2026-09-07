# The Wrapping Paper 🎁 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-gift and per-shelf chat unfurls — `/l/<token>` and `/s/<token>` serve the deployed SPA `index.html` with a personalized OpenGraph block swapped in at the origin, so the link Ben pastes unfurls as a wrapped present for THAT friend.

**Architecture:** CloudFront gains two API-origin behaviors (`/l/*`, `/s/*`); API Gateway gains matching proxy paths; public-api serves the web bucket's `index.html` (cached ~60s) with the og block replaced via explicit HTML comment markers. One dynamo read per unfurl (`get_link` / `get_friend_by_shelf_token`). Wrap art = 8 deterministic gift PNGs (token-hashed) + 1 shelf PNG, static web assets.

**Tech Stack:** Rust (axum, lambda_http, aws-sdk-s3 NEW dep), existing dynamo store, Terraform (CF module `ordered_cache_behaviors` with `cache_policy_id`), Vite/React web (markers + assets only), vitest, moto for S3 integration.

**Spec:** `docs/spec-wrapping-paper.md` (r2.4, family-signed 2026-09-07 — the plan argues from it; executors read both)

## Global Constraints

- Identical bytes to every fetcher of a path — NO UA-conditional responses (spec non-goals).
- The unfurl is the wrapped box, never the contents: no game titles/art in meta, count at most.
- Personalized states: `active` and `sealed` ONLY. `revoked|expired|exhausted|unknown` → the template's own generic og block, byte-identical to what S3 serves today (spec D5).
- Sealed copy carries NO date: `sealed for now. good things wait.` (spec D6).
- All injected text passes `domain::og_text` (strip control+format chars → clamp → attribute-escape `& < > " '`); `"` is the breakout char. Property: hostile label CANNOT alter tag structure.
- Marker absence = ERROR log + EMF metric `UnfurlMarkerAbsent`, then serve generic (spec D3 witness).
- The escaper lives ONCE, in `crates/domain`. `is_spoofing_format_char` MOVES from public-api to domain (re-used there); admin-api's copy is untouched this arc — no third implementation.
- Wrap hash: FNV-1a 64 over the token bytes, `% 8` — deterministic forever; golden test pins it.
- Cache: CF custom cache policy default/max TTL 60s on both behaviors; lambda sends `Cache-Control: public, max-age=60` on personalized 200s, and `no-store` never (that's the JSON API's sealed rule, not ours — our sealed card is stable copy).
- Commits: signed (`-S`), conventional-ish gift-shelf style, author `code kitten <yourcodekitten@gmail.com>`.
- Heavy builds ride CI; locally run only `cargo test -p <crate>` (box linker constraint).

## File Structure

- `crates/domain/src/lib.rs` — add `og_text`, `is_spoofing_format_char` (moved in), unit tests.
- `crates/public-api/src/lib.rs` — remove local `is_spoofing_format_char` (use domain's); add unfurl module wiring: `TemplateSource` trait + `swap_og_block` + meta builders + 2 handlers + 2 routes; `router()` KEEPS its 4-arg signature — `router_with_template` carries the 5th param (Task 3).
- `crates/public-api/src/unfurl.rs` — NEW: everything unfurl (trait, S3 impl, cache, swap, meta, hash). Keeps lib.rs from growing another 400 lines.
- `crates/public-api/src/main.rs` — wire S3 client from `WEB_BUCKET` env.
- `crates/public-api/Cargo.toml` — add `aws-sdk-s3` (same feature shape as the other SDK deps).
- `crates/public-api/tests/unfurl_test.rs` — NEW: handler integration (store_or_skip harness).
- `crates/public-api/tests/unfurl_moto.rs` — NEW: moto S3 integration.
- `web/index.html` — og markers around the existing block.
- `web/src/App.test.tsx` (or new `web/src/ogMarkers.test.ts`) — marker contract test.
- `web/public/art/wrap-{clay,rust,mustard,moss,pine,slate,heather,mauve}.png`, `web/public/art/wrap-shelf.png` — NEW assets (pre-staged by the main session, see Task 4).
- `terraform/aws-apigateway.tf` — `/l/{proxy+}` + `/s/{proxy+}` paths.
- `terraform/aws-cloudfront.tf` — cache policy resource + 2 ordered behaviors.
- `terraform/aws-lambda.tf` — `WEB_BUCKET` env + `s3` inline policy on public-api.
- `docs/spec-wrapping-paper.md` — r2.5 correction (Task 5) + BUILT flip (Task 6).
- `DESIGN.md` — wrap-art palette note (Task 6).

---

### Task 1: domain — `og_text` sanitizer/escaper (single home)

**Files:**
- Modify: `crates/domain/src/lib.rs` (append at end, before tests mod if one exists)
- Modify: `crates/public-api/src/lib.rs` — cut `fn is_spoofing_format_char` WHOLE: from its doc
  comment through its closing `}` (**20 match arms; first `'\u{00AD}'`, last
  `'\u{E0000}'..='\u{E007F}'` — count them after the cut**). Cite-by-symbol, not by line: the
  file is 1543 lines and this task edits it.

**Interfaces:**
- Produces: `pub fn domain::is_spoofing_format_char(c: char) -> bool` (moved, body verbatim — the whole 20-arm `matches!`) and `pub fn domain::og_text(raw: &str, max_chars: usize) -> String` — strips `char::is_control` and spoofing format chars, clamps to `max_chars` chars (char-boundary safe), then escapes `& < > " '` in that order (`&` first). Later tasks call `og_text(label, 80)`.

- [ ] **Step 1: Write the failing tests** (in `crates/domain/src/lib.rs` tests mod)

```rust
#[test]
fn og_text_escapes_attribute_breakers() {
    assert_eq!(domain_crate_name::og_text(r#"a"b<c>d&e'f"#, 80),
        "a&quot;b&lt;c&gt;d&amp;e&#39;f");
}

#[test]
fn og_text_strips_control_and_format_chars() {
    // \u{202E} RLO is the bidi spoof char; \u{0007} is control
    assert_eq!(domain_crate_name::og_text("a\u{202E}b\u{0007}c", 80), "abc");
}

#[test]
fn og_text_clamps_on_char_boundaries() {
    assert_eq!(domain_crate_name::og_text("héllo", 3), "hél");
}

#[test]
fn og_text_hostile_label_cannot_change_tag_structure() {
    // Structure-invariance (Lilith): render into the exact attribute template and
    // assert the parsed tag count and attribute value survive.
    let hostile = r#"" onload=x><script>alert(1)</script>"#;
    let content = domain_crate_name::og_text(hostile, 200);
    let tag = format!(r#"<meta property="og:title" content="{content}" />"#);
    // No new element boundaries: exactly one '<' and one '>' pair belonging to
    // the meta tag itself; the escaped payload contributes zero raw < > ".
    assert_eq!(tag.matches('<').count(), 1);
    assert_eq!(tag.matches('>').count(), 1);
    assert_eq!(tag.matches('"').count(), 4); // the four template quotes only
}
```
(Use the crate's real name in place of `domain_crate_name` — check `crates/domain/Cargo.toml` `[package] name`.)

- [ ] **Step 2: Run to verify failure** — `cargo test -p domain og_text` → FAIL: function not found.
- [ ] **Step 3: Implement** (append to `crates/domain/src/lib.rs`):

```rust
/// True for Unicode format chars that can visually spoof text (bidi controls,
/// zero-widths). MOVED VERBATIM from public-api (which now re-uses this) —
/// admin-api holds a deliberate second copy (see its :997 sync note); this
/// move keeps the count at two, adding no third.
pub fn is_spoofing_format_char(c: char) -> bool {
    // BODY COMES FROM THE CUT — do Step 3 in this order: FIRST locate
    // `fn is_spoofing_format_char` in crates/public-api/src/lib.rs and CUT the
    // ENTIRE function (doc comment through closing brace — 20 match arms; a
    // partial cut compiles once the parens balance and silently shrinks the
    // spoof set, so verify the arm count in the paste). Step 3 CUTS AND PASTES
    // ONLY — the import and public-api's green run belong to Step 5, one owner
    // per edit
    // THEN paste that body here verbatim. The plan deliberately does not
    // restate the body: a restated body is a recollection wearing source's
    // clothes, and cut-first makes keeping a guessed body impossible —
    // public-api will not compile until the paste lands (review MAJOR-2).
    unimplemented!("replaced by the cut body in the same step")
}

/// Sanitize + attribute-escape text bound for an HTML attribute (og meta
/// content). Strip → clamp → escape, in that order; `&` escapes first.
/// `"` is the breakout character for attribute context — never skip it.
pub fn og_text(raw: &str, max_chars: usize) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !is_spoofing_format_char(*c))
        .take(max_chars)
        .collect();
    let mut out = String::with_capacity(cleaned.len());
    for c in cleaned.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
```

- [ ] **Step 4:** `cargo test -p domain` → PASS (all four + existing suite).
- [ ] **Step 5:** The cut already happened in Step 3 (whole symbol, doc comment through closing
brace). Here: add `use domain::is_spoofing_format_char;` near the other domain imports in
`crates/public-api/src/lib.rs` (grep `use domain::` for the exact block), then
`cargo test -p public-api` → PASS (its existing sanitize tests, e.g. the :1108 caller, now
exercise the moved fn).
- [ ] **Step 6: Commit** — `git add -A && git commit -S -m "🎁 domain: og_text attribute escaper; is_spoofing_format_char moves to its single home"`

### Task 2: public-api — `swap_og_block` + meta builders (pure core)

**Files:**
- Create: `crates/public-api/src/unfurl.rs`
- Modify: `crates/public-api/src/lib.rs` (add `mod unfurl;` + `pub use` for tests)

**Interfaces:**
- Consumes: `domain::og_text`, `domain::Link` (fields `label: String`, `curated_game_ids: Option<Vec<String>>`, `can_claim(now) -> Result<(), ClaimRefusal>`), `domain::Friend` (field `name: String`).
- Produces (Task 3 relies on these exact names):
  - `pub(crate) const OG_BEGIN: &str = "<!-- og:begin -->";` / `OG_END`
  - `pub(crate) fn swap_og_block(template: &str, meta_html: &str) -> Result<String, MarkerAbsent>` (`pub(crate) struct MarkerAbsent;`)
  - `pub(crate) fn wrap_variant(token: &str) -> &'static str` → one of `"clay","rust","mustard","moss","pine","slate","heather","mauve"` via FNV-1a64 % 8 (order exactly as listed — golden-pinned)
  - `pub(crate) fn meta_for_link(link: &domain::Link, now: time::OffsetDateTime, base_url: &str) -> Option<String>` — `Some(meta_html)` for active/sealed, `None` for every dead state
  - `pub(crate) fn meta_for_shelf(friend: &domain::Friend, base_url: &str) -> String`

- [ ] **Step 1: failing tests** (in `unfurl.rs` `#[cfg(test)] mod tests`):

```rust
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
    assert_eq!(wrap_variant("8af17e0500caf83ec1172baf05a661ba1ee4ab02114f643b4ab5d2efe9ed80ec"), "clay");
    assert_eq!(wrap_variant("b8b7c23ab0e0c45567869d4c98c9d489c8000a21da777c54e4cdeba61a513957"), "rust");
    assert_eq!(wrap_variant("a0a8bddef089f638f98ca3f13aead6a96aaf25955eaeaa1a614c9a38427e6092"), "mustard");
    let all: std::collections::HashSet<_> =
        (0..64).map(|i| wrap_variant(&format!("{i:064x}"))).collect();
    assert!(all.len() >= 6, "64 tokens should hit most of 8 buckets: got {}", all.len());
}

#[test]
fn swap_og_block_replaces_between_markers() {
    let t = "head\n<!-- og:begin -->\nOLD\n<!-- og:end -->\ntail";
    let out = swap_og_block(t, "NEW").unwrap();
    assert!(out.contains("NEW") && !out.contains("OLD"));
    assert!(out.starts_with("head\n") && out.ends_with("\ntail"));
}

#[test]
fn swap_og_block_absent_marker_is_typed() {
    assert!(swap_og_block("no markers here", "X").is_err());
}

#[test]
fn meta_for_link_active_curated_counts_in_words() {
    let mut link = test_link(); // build with the same helper style store tests use
    link.curated_game_ids = Some(vec!["a".into(), "b".into(), "c".into()]);
    let m = meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").unwrap();
    assert!(m.contains("three treasures inside, chosen for you. tap to unwrap."));
    assert!(m.contains("ben wrapped something for"));
    assert!(m.contains("/art/wrap-")); // one of the 8
}

#[test]
fn meta_for_link_sealed_has_state_but_never_a_clock() {
    let mut link = test_link();
    link.unlock_at = Some(time::OffsetDateTime::now_utc() + time::Duration::days(2));
    let m = meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").unwrap();
    assert!(m.contains("sealed for now. good things wait."));
    let year = time::OffsetDateTime::now_utc().year().to_string();
    assert!(!m.contains(&year), "no date-ish content in a broadcast card");
}

#[test]
fn meta_for_link_dead_states_are_none() {
    let mut link = test_link();
    link.revoked = true;
    assert!(meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").is_none());
}

#[test]
fn hostile_base_url_cannot_change_structure_either() {
    // base_url is server config today (router()'s 4th param) — this arm exists
    // for the future refactor that derives it from a request.
    let link = test_link();
    let m = meta_for_link(&link, time::OffsetDateTime::now_utc(), r#"https://x"><script>"#).unwrap();
    for line in m.lines().filter(|l| !l.trim().is_empty()) {
        assert_eq!(line.trim().matches('<').count(), 1, "injected < in: {line}");
    }
}

#[test]
fn hostile_label_cannot_change_structure_at_the_meta_layer() {
    let mut link = test_link();
    link.label = r#"" onload=x><script>"#.into();
    let m = meta_for_link(&link, time::OffsetDateTime::now_utc(), "https://x.example").unwrap();
    // No raw < > outside tag boundaries: every line parses as a <meta …/> element.
    for line in m.lines().filter(|l| !l.trim().is_empty()) {
        let l = line.trim();
        assert!(l.starts_with("<meta ") && l.ends_with("/>"), "unexpected line: {l}");
        assert_eq!(l.matches('<').count(), 1, "injected < in: {l}");
    }
}
```
(`test_link()`: construct a `domain::Link` the way `crates/dynamo` store tests do — grep a
literal `Link {` in the dynamo tests and mirror the minimal valid struct. Field names verified
against `domain:183-262` at review: `revoked: bool` · `unlock_at: Option<OffsetDateTime>` ·
`curated_game_ids: Option<Vec<String>>` — as written above, no adjustment needed.)

- [ ] **Step 2:** `cargo test -p public-api unfurl` → FAIL (module absent).
- [ ] **Step 3: implement `unfurl.rs` core:**

```rust
//! The wrapping paper: per-token og meta swapped into the deployed index.html.
//! Spec: docs/spec-wrapping-paper.md (r2). The card is the wrapped box, never
//! the contents; personalized ONLY for active|sealed; audience = the room.
use domain::og_text;

pub(crate) const OG_BEGIN: &str = "<!-- og:begin -->";
pub(crate) const OG_END: &str = "<!-- og:end -->";

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

const WRAPS: [&str; 8] = ["clay", "rust", "mustard", "moss", "pine", "slate", "heather", "mauve"];

pub(crate) fn wrap_variant(token: &str) -> &'static str {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in token.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    WRAPS[(h % 8) as usize]
}

fn count_words(n: usize) -> String {
    const W: [&str; 12] = ["one","two","three","four","five","six","seven","eight","nine","ten","eleven","twelve"];
    if (1..=12).contains(&n) { W[n - 1].to_string() } else { "a dozen and more".to_string() }
}

fn meta_block(title: &str, desc: &str, image: &str, alt: &str) -> String {
    // INVARIANT (tested, not asserted): every interpolated value has passed
    // og_text — including image URLs (escaping a URL is harmless and correct in
    // attribute context; &→&amp;) — so a future base_url-from-Host refactor
    // cannot bypass the escaper silently (Lilith's MAJOR-1).
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
                format!("{} treasure{s} inside, chosen for you. tap to unwrap.", count_words(n))
            }
            _ => "treasures inside, chosen for you. tap to unwrap.".to_string(),
        }
    };
    let image = og_text(&format!("{base_url}/art/wrap-{}.png", wrap_variant(&link.token)), 200);
    Some(meta_block(&title, &og_text(&desc, 200), &image,
        "a pixel-art wrapped present in ben's pea-green attic"))
}

pub(crate) fn meta_for_shelf(friend: &domain::Friend, base_url: &str) -> String {
    let name = og_text(&friend.name, 80);
    meta_block(
        &format!("📚 the shelf ben keeps for {name}"),
        "every game he's given you, all in one warm place.",
        &og_text(&format!("{base_url}/art/wrap-shelf.png"), 200),
        "a pixel-art shelf of games in ben's attic",
    )
}
```
Add `mod unfurl;` in `lib.rs` (top, near other mods).

- [ ] **Step 4:** `cargo test -p public-api unfurl` → PASS. Fix the `test_link` revocation field per the real domain struct while here.
- [ ] **Step 5: Commit** — `git commit -S -m "🎁 public-api: unfurl core — marker swap, pinned wrap hash, meta builders (structure-invariant)"`

### Task 3: public-api — TemplateSource, S3 fetch + cache, handlers, routes

**Files:**
- Modify: `crates/public-api/src/unfurl.rs` (append), `crates/public-api/src/lib.rs` (router + AppState), `crates/public-api/src/main.rs`, `crates/public-api/Cargo.toml`
- Create: `crates/public-api/tests/unfurl_test.rs` (handler integration, Step 1)
- Create: `crates/public-api/tests/unfurl_moto.rs` (S3 template integration, Step 5)

**Interfaces:**
- Consumes (from Task 2, exact signatures inlined — the executor sees only this block):
  - `pub(crate) const OG_BEGIN: &str = "<!-- og:begin -->";` and `OG_END: &str = "<!-- og:end -->";`
  - `pub(crate) fn swap_og_block(template: &str, meta_html: &str) -> Result<String, MarkerAbsent>`
    (`pub(crate) struct MarkerAbsent;`)
  - `pub(crate) fn wrap_variant(token: &str) -> &'static str`
  - `pub(crate) fn meta_for_link(link: &domain::Link, now: time::OffsetDateTime, base_url: &str) -> Option<String>`
    (`None` = dead state ⇒ serve the template unmodified)
  - `pub(crate) fn meta_for_shelf(friend: &domain::Friend, base_url: &str) -> String`
- Produces: `#[async_trait] pub trait TemplateSource: Send + Sync { async fn fetch(&self) -> Result<String, TemplateError>; }` — VERIFIED at review: the crate already uses `async_trait` (lib.rs:9, `Invoker` at :27); mirror that idiom, no new dep. `pub struct S3Template { … }` with `pub fn new(client: aws_sdk_s3::Client, bucket: String) -> Self`, 60s in-memory cache (`tokio::sync::RwLock<Option<(std::time::Instant, String)>>`). **`router()` KEEPS its 4-arg signature** (103 existing call sites, 95 in api_test.rs — measured;
breaking it is 103 mechanical edits for nothing). Add
`pub fn router_with_template(store, invoker, steam, base_url, template: Option<std::sync::Arc<dyn TemplateSource>>) -> Router`
holding the real body; `router(a,b,c,d)` becomes a one-line delegate passing `None`. Only main.rs
and the new unfurl tests call the 5-arg form.

- [ ] **Step 1: failing handler tests** — Create `crates/public-api/tests/unfurl_test.rs`,
**mirroring `tests/api_test.rs` EXACTLY** (same imports, same `store_or_skip` helper copied in,
same MockInvoker; api_test.rs is UNTOUCHED — `router()` keeps its arity, these tests call
`router_with_template`). Full file skeleton:

```rust
//! Unfurl route integration tests. Store-backed via store_or_skip (CI runs them,
//! local skips without dynamodb-local) — see api_test.rs, whose harness this mirrors.
use std::sync::Arc;
use async_trait::async_trait;
use axum::{body::Body, http::{Request, StatusCode}};
use public_api::{router_with_template, TemplateSource, TemplateError};
use tower::ServiceExt;

const TPL: &str = "<html><head>\n<!-- og:begin -->\nGENERIC\n<!-- og:end -->\n</head><body></body></html>";

struct StubTemplate(String);
#[async_trait]
impl TemplateSource for StubTemplate {
    async fn fetch(&self) -> Result<String, TemplateError> { Ok(self.0.clone()) }
}
struct FailingTemplate;
#[async_trait]
impl TemplateSource for FailingTemplate {
    async fn fetch(&self) -> Result<String, TemplateError> { Err(TemplateError::Unavailable) }
}

// copy store_or_skip + MockInvoker + the link-seeding helper from api_test.rs verbatim,
// then:

#[tokio::test]
async fn unfurl_link_serves_personalized_html_with_cache_header() {
    let Some(store) = store_or_skip("unfurl-personalized").await else { return };
    // seed an ACTIVE link with curated_game_ids = ["a","b","c"] the way api_test.rs
    // seeds links (mirror its seeding helper; token = a fixed 64-hex literal)
    let app = router_with_template(store, mock_invoker(), None, "https://x.example".into(),
        Some(Arc::new(StubTemplate(TPL.into()))));
    let res = app.oneshot(Request::builder().uri("/l/<the-token>").body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/html; charset=utf-8");
    assert_eq!(res.headers()["cache-control"], "public, max-age=60");
    let body = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(body.contains("ben wrapped something for"));
    assert!(body.contains("three treasures inside"));
    assert!(!body.contains("GENERIC"));
}

#[tokio::test]
async fn unfurl_dead_token_serves_template_byte_identical() {
    let Some(store) = store_or_skip("unfurl-dead").await else { return };
    let app = router_with_template(store, mock_invoker(), None, "https://x.example".into(),
        Some(Arc::new(StubTemplate(TPL.into()))));
    let res = app.oneshot(Request::builder().uri(&format!("/l/{}", "f".repeat(64))).body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert_eq!(body, TPL, "generic card must be the template verbatim (spec D5)");
}

#[tokio::test]
async fn unfurl_template_without_markers_serves_generic_and_is_loud() {
    let Some(store) = store_or_skip("unfurl-markerless").await else { return };
    // seed a VALID active link; template WITHOUT markers
    let app = router_with_template(store, mock_invoker(), None, "https://x.example".into(),
        Some(Arc::new(StubTemplate("<html>no markers</html>".into()))));
    let res = app.oneshot(Request::builder().uri("/l/<the-token>").body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = String::from_utf8(axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert_eq!(body, "<html>no markers</html>");
    // the EMF witness: unit-tested by shape in unfurl.rs (emf_blob_for_marker_absent
    // test asserting the JSON parses and carries Namespace bendobundles/unfurl +
    // metric UnfurlMarkerAbsent=1) — the handler path here proves the DEGRADE half.
}

#[tokio::test]
async fn unfurl_source_failure_is_500_json() {
    let Some(store) = store_or_skip("unfurl-fail").await else { return };
    let app = router_with_template(store, mock_invoker(), None, "https://x.example".into(),
        Some(Arc::new(FailingTemplate)));
    let res = app.oneshot(Request::builder().uri(&format!("/l/{}", "a".repeat(64))).body(Body::empty()).unwrap())
        .await.unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
```
(`mock_invoker()` and the seeding helper: copy api_test.rs's forms; `<the-token>` = the literal
you seeded. `TemplateSource`/`TemplateError` must be `pub` and exported from lib.rs for this
file — add to the crate's public exports in Step 3.)

- [ ] **Step 2:** run → FAIL (trait absent).
- [ ] **Step 3: implement.** Key pieces:

```rust
pub enum TemplateError { Unavailable }

pub struct S3Template {
    client: aws_sdk_s3::Client,
    bucket: String,
    cache: tokio::sync::RwLock<Option<(std::time::Instant, String)>>,
}

impl S3Template {
    const TTL: std::time::Duration = std::time::Duration::from_secs(60);
    pub fn new(client: aws_sdk_s3::Client, bucket: String) -> Self { /* … */ }
    async fn fetch_inner(&self) -> Result<String, TemplateError> {
        if let Some((at, s)) = self.cache.read().await.as_ref() {
            if at.elapsed() < Self::TTL { return Ok(s.clone()); }
        }
        let out = self.client.get_object().bucket(&self.bucket).key("index.html")
            .send().await.map_err(|_| TemplateError::Unavailable)?;
        let bytes = out.body.collect().await.map_err(|_| TemplateError::Unavailable)?;
        let s = String::from_utf8(bytes.into_bytes().to_vec()).map_err(|_| TemplateError::Unavailable)?;
        *self.cache.write().await = Some((std::time::Instant::now(), s.clone()));
        Ok(s)
    }
}
```

Handlers (axum, in unfurl.rs; wire from lib.rs routes `/l/{token}` and `/s/{token}`):
- fetch template (None source or Err → 500 JSON `{"error":"try again"}`);
- dynamo read (`get_link` / `get_friend_by_shelf_token`); `Ok(None)`/dead → serve template UNMODIFIED with `text/html` + `Cache-Control: public, max-age=60` (generic card, 200 — spec D5);
- meta build → `swap_og_block`; `Err(MarkerAbsent)` → `tracing::error!` + EMF line to stdout (`println!` of the serde_json EMF blob — namespace `bendobundles/unfurl`, metric `UnfurlMarkerAbsent`, value 1) → serve template unmodified;
- success → swapped HTML, `text/html; charset=utf-8`, `Cache-Control: public, max-age=60`.

`Cargo.toml`: `aws-sdk-s3 = { version = "1", default-features = false, features = ["default-https-client", "rt-tokio"] }` (mirror line 18's shape).
`main.rs`: read `WEB_BUCKET` env; when present build `aws_sdk_s3::Client` from the same shared `aws_config` the other clients use and pass `Some(Arc::new(S3Template::new(...)))`, else `None`.
`router()` signature: add the param; update every call site (`grep -n 'router(' crates/public-api` — tests pass `None`, the four new tests pass stubs).

- [ ] **Step 4:** `cargo test -p public-api` → PASS entire crate.
- [ ] **Step 5: moto integration** (`tests/unfurl_moto.rs`): mirror the moto harness style from `crates/dynamo` tests (grep `moto` there for endpoint/env conventions): create bucket, put an `index.html` WITH markers, build `S3Template` against the moto endpoint, assert fetch returns it, overwrite the object, assert the cached copy survives within TTL (fetch again immediately → OLD bytes; this pins the cache semantics). Run: `cargo test -p public-api --test unfurl_moto` (moto on :8000 per repo convention — kill it after).
- [ ] **Step 6: Commit** — `git commit -S -m "🎁 public-api: unfurl routes — S3 template with 60s cache, loud marker witness, one dynamo read per card"`

### Task 4: web — markers, marker contract test, wrap art

**Files:**
- Modify: `web/index.html:21-42` (og block :21-33 AND twitter block :35-42 — both go inside the markers)
- Create: `web/src/ogMarkers.test.ts`
- Create: `web/public/art/wrap-*.png` (9 files, pre-staged — see Step 4)

**Interfaces:**
- Produces: `index.html` og AND twitter blocks (:21-42) wrapped in `<!-- og:begin -->` / `<!-- og:end -->` EXACTLY (Task 2's constants); art filenames exactly `wrap-clay.png … wrap-mauve.png`, `wrap-shelf.png` (Task 2's `WRAPS` order/names).

- [ ] **Step 1: failing test** (`web/src/ogMarkers.test.ts`):

```ts
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The lambda swaps the og block anchored on these exact markers (spec D3).
// This test is the BUILD-side half of the contract; the lambda's marker
// witness is the bucket-side half. Change one, change both.
describe("og markers", () => {
  const html = readFileSync(resolve(__dirname, "../index.html"), "utf8");
  it("carries begin/end markers in order, once each", () => {
    const b = html.indexOf("<!-- og:begin -->");
    const e = html.indexOf("<!-- og:end -->");
    expect(b).toBeGreaterThan(-1);
    expect(e).toBeGreaterThan(b);
    expect(html.indexOf("<!-- og:begin -->", b + 1)).toBe(-1);
    expect(html.indexOf("<!-- og:end -->", e + 1)).toBe(-1);
  });
  it("wraps the og block (og:title inside the markers)", () => {
    const inner = html.slice(html.indexOf("<!-- og:begin -->"), html.indexOf("<!-- og:end -->"));
    expect(inner).toContain('property="og:title"');
    expect(inner).toContain('name="twitter:image"'); // the :35-42 block must be INSIDE
  });
  it("ships all nine wrap art assets", () => {
    for (const v of ["clay","rust","mustard","moss","pine","slate","heather","mauve","shelf"]) {
      expect(() => readFileSync(resolve(__dirname, `../public/art/wrap-${v}.png`))).not.toThrow();
    }
  });
});
```

- [ ] **Step 2:** `npx vitest run src/ogMarkers.test.ts` (from `web/`) → FAIL.
- [ ] **Step 3:** Edit `web/index.html`: insert `<!-- og:begin -->` on the line BEFORE the
`<!-- open graph … -->` comment (:21) and `<!-- og:end -->` AFTER the twitter block's last tag
(`twitter:image`, :42) — **VERIFIED at review: the twitter card block lives at :35-42, OUTSIDE the
og block. Both blocks must sit inside the markers**, or a personalized page ships generic
`twitter:title`/`twitter:image` beside the personalized og and twitter-preferring unfurlers show
the generic card. The swap replaces the whole span; `meta_block` (Task 2) emits the full
og+twitter set for exactly this reason.
- [ ] **Step 4 (MAIN SESSION, not a subagent — billable art + judgment):** stage the 9 PNGs. The master is `<scratchpad>/wrap-art/wrap-master-final.png` (1216×640, high quality, generated 07:14). Recipe, already proven on the draft:

```bash
cd <scratchpad>/wrap-art
# re-sample the FINAL master's bow (it may differ from the draft's):
convert wrap-master-final.png -crop 24x24+<bow-x>+<bow-y> +repage -resize 1x1 txt:-
# recompute per-token (L%,S%,hue-arg) with the python block from the session
# (targets: index.css:34-41 oklch values → sRGB), then per variant:
convert wrap-master-final.png -modulate "$L,$S,$H" shift.png
convert wrap-master-final.png shift.png mask.png -compose Over -composite full.png
convert full.png -gravity center -crop 1200x630+0+0 +repage wrap-<name>.png
# rust variant = the master itself, center-cropped (its bow IS rust).
# shelf: separate gpt-image edit of the master — same room, present replaced by a
#   small wooden shelf of game boxes, NO accent color pop (reads as furniture,
#   disjoint-palette rule) — then center-crop identically.
pngquant/optipng if available; copy all 9 into web/public/art/.
```
Eyeball all 9 at once before committing (montage). Muted is correct; illegible is not.

- [ ] **Step 5:** `npx vitest run` (full web suite) → PASS.
- [ ] **Step 6: Commit** — `git commit -S -m "🎁 web: og markers + marker contract test + nine wrapping papers (muted-earth, token-pinned)"`

### Task 5: terraform — gateway paths, behaviors, cache policy, IAM, env

**Files:**
- Modify: `terraform/aws-apigateway.tf` (paths map), `terraform/aws-cloudfront.tf` (cache policy + behaviors), `terraform/aws-lambda.tf` (public-api env + policy)
- Modify: `docs/spec-wrapping-paper.md` (r2.5 correction)

**Interfaces:**
- Consumes: web bucket = `module.site.s3_bucket_id` / `.s3_bucket_arn` (aws-cloudfront.tf:112 module).
- Produces: infra reaching Task 3's lambda at `/l/*` + `/s/*` with `WEB_BUCKET` set.

- [ ] **Step 1:** `aws-apigateway.tf`: add two path entries to the OpenAPI `paths` map, copying the `"/api/{proxy+}"` block verbatim with keys `"/l/{proxy+}"` and `"/s/{proxy+}"`, both targeting `module.lambda_public_api.lambda_function_arn` (GET-only is tempting but keep ANY-method parity with the siblings — the lambda 404s the rest).
- [ ] **Step 2:** `aws-cloudfront.tf`: add

```hcl
module "label_unfurl_cache" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "unfurl-cache"
}

# 60s shared cache for personalized unfurl HTML (spec D4/OQ2): per-path keys —
# the token is the path; DECIDED cost: up to 60s of revocation latency on the
# card (the page itself stays live-checked). No cookies/headers/query in key.
resource "aws_cloudfront_cache_policy" "unfurl" {
  name        = module.label_unfurl_cache.id
  default_ttl = 60
  max_ttl     = 60
  min_ttl     = 0
  parameters_in_cache_key_and_forwarded_to_origin {
    cookies_config       { cookie_behavior = "none" }
    headers_config       { header_behavior = "none" }
    query_strings_config { query_string_behavior = "none" }
    enable_accept_encoding_gzip   = true
    enable_accept_encoding_brotli = true
  }
}
```

and extend `ordered_cache_behaviors` (AFTER the two api rows — order is evaluation order):

```hcl
    { path_pattern = "/l/*", target_origin_id = "api",
      allowed_methods = ["GET", "HEAD"], cached_methods = ["GET", "HEAD"],
      cache_policy_id = aws_cloudfront_cache_policy.unfurl.id },
    { path_pattern = "/s/*", target_origin_id = "api",
      allowed_methods = ["GET", "HEAD"], cached_methods = ["GET", "HEAD"],
      cache_policy_id = aws_cloudfront_cache_policy.unfurl.id },
```

- [ ] **Step 3:** `aws-lambda.tf` public-api module: env gains `WEB_BUCKET = module.site.s3_bucket_id` (comment: template for unfurl HTML — spec D3); `addl_inline_policies` gains, mirroring the ssm entry's hand-written shape:

```hcl
    web_index = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect   = "Allow"
        Action   = ["s3:GetObject"]
        Resource = ["${module.site.s3_bucket_arn}/index.html"]
      }]
    })
```

- [ ] **Step 4 (spec r2.5):** in `docs/spec-wrapping-paper.md` D3, replace the iam_capture sentence: the corpus is DYNAMO-scoped by design (`crates/dynamo/tests/iam_capture.rs:1` captures x-amz-target request shapes); non-dynamo grants follow the hand-written inline-policy pattern (`aws-lambda.tf` ssm precedent) — this S3 grant does the same, single object, no wildcard.
- [ ] **Step 5:** `terraform fmt -check terraform/` → clean. READ the three diffs line-by-line (no local plan — deploy-time plan is the gate, runbook #217: infra-change arc, non-zero destroy = read-every-line).
- [ ] **Step 6: Commit** — `git commit -S -m "🎁 terraform: /l/* + /s/* to the api origin, 60s unfurl cache policy, WEB_BUCKET + single-object s3 read"`

### Task 6: docs — spec flip + DESIGN.md palette note

**Files:**
- Modify: `docs/spec-wrapping-paper.md` (status line), `DESIGN.md`

- [ ] **Step 1:** spec status → `BUILT (2026-09-07) — r2 as reviewed; see PR`.
- [ ] **Step 2:** `DESIGN.md`, after the Title-Hash Rule section, add:

```markdown
**The Wrapping-Paper Rule.** Gift unfurl art (`web/public/art/wrap-*.png`) draws its accent from
the same muted-earth tokens as the title-hash palette, keyed by FNV-1a64(link token) % 8 — the
same gift wears the same paper forever (re-pastes unfurl identically). The shelf card is ONE
design with no accent pop: a shelf is a different KIND of object, not another present, and a
disjoint look takes the shelf-matches-a-gift-paper collision to zero by construction
(1−(7/8)ⁿ ≈ 33% at three gifts if it were hashed — spec D1). Changing the hash, the bucket
order, or the filenames breaks the promise; don't.
```

- [ ] **Step 3:** `npx vitest run` (web) + `cargo test -p public-api -p domain` one last local pass → PASS.
- [ ] **Step 4: Commit** — `git commit -S -m "🎁 docs: spec flipped BUILT; DESIGN.md learns the wrapping-paper rule"`

---

## Product ruling recorded (Lilith's review question)
Sealed outranks Expired/Exhausted (`domain:319`), so a sealed-and-expired link unfurls
"sealed for now" warm — INTENDED: it mirrors the JSON API's own ranking exactly (no new oracle),
and Revoked outranks Sealed, so Ben can silence any dead sealed link by revoking it. The card
never promises a claim, only warmth.

## Self-review notes (run at plan time, kept for the executor)
- Spec coverage: D1→T2/T4/T6 · D2→(non-goal, no task) · D3→T3/T4/T5(+r2.5) · D4→T5 · D5→T3 (dead→byte-identical test) · D6→T2 (copy verbatim in builders + tests) · witness→T3/T4 · resolutions→T5 cache policy comment.
- The two plan-marked EXECUTOR notes (predicate body verbatim; revocation field name) are deliberate read-the-source pins, not placeholders — the source outranks the plan's recollection.
- Type consistency: `wrap_variant` names == art filenames == vitest list == WRAPS order (single source: this plan, golden-pinned in T2, contract-tested in T4).
- Review-verified facts (2026-09-07 plan review): `#[async_trait]` idiom confirmed (public-api:9/:27) · `aws_lambda_permission` `source_arn` ends `/*/*/*` — multi-segment wildcard covers `/l/*`+`/s/*`, no new permission needed · twitter block at index.html:35-42 is OUTSIDE the og block (drove BLOCKER-1's marker-span + meta_block fixes) · `is_spoofing_format_char` has exactly one public-api use (:1108), clean move.
