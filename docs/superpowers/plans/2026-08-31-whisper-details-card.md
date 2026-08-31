# Whisper v2: Full Details Card — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The weekly whisper carries the full friend-visible details card (fields + all media) as Discord webhook embeds, plus a zero-write `whisper_preview` envelope.

**Architecture:** A pure, I/O-free card builder in `crates/fulfillment/src/whisper.rs` returns the complete webhook JSON body; `lib.rs` gains a JSON-body deliver twin, one steam-cache read in `handle_whisper`, and a new `WhisperPreview` envelope that re-renders without writing. Selection, idempotence, and all five no-send causes are untouched.

**Tech Stack:** Rust (fulfillment crate), serde_json, wiremock for integration tests, dynamo `SteamAppCache` read via existing `Deps.store`.

**Spec:** `docs/spec-whisper-details-card.md` (this repo). Field inventory measured from `web/src/GameDetailModal.tsx` @ e87aad7.

## Global Constraints

- Discord limits (constants in whisper.rs): embed title ≤256 · description ≤4096 · field value ≤1024 · ≤10 embeds · combined embed TEXT ≤6000 chars (urls don't count) · gallery = ≤4 embeds sharing one identical `url`. **The gallery grouping is CLIENT RENDERING, not API contract** (the 10-embed cap is documented; the merge is not) — known-fragile, degrades to a tall column, shipped with that grade written down (spec §provenance).
- `"allowed_mentions": {"parse": []}` on EVERY payload shape — load-bearing (#174). **Two protection layers, named (OMBB):** `parse:[]` is documented for CONTENT; embed text is protected by embeds-don't-render-mentions, an undocumented behavior. Comments must not claim the field covers embeds.
- **Media completeness is the requirement Ben stated in words:** all 10 screenshots ship (embed[0] carries screenshots[0]; header art lives in the thumbnail chain). A test asserting a silent drop is a spec violation, not a pin (family round 1, Lilith's ①).
- Any truncation (title/description/fields) appends `" · trimmed to fit"` to the footer — drops announce themselves in production, not only in tests.
- Content descriptors (`content_descriptor_ids`/`content_notes`) are admin-only (#71) — MUST NOT appear in any whisper payload.
- The whisper voice: lowercase, ♡, friend register. Content line keeps v1's wording minus the bare art URL (the embed carries the art now; a bare URL would double-render).
- No terraform / IAM / schedule changes. `cargo fmt` + `cargo clippy --workspace --all-targets` + full `cargo test --workspace` green before PR (moto_server on :8000 for the dynamo suites; CI runs the same).
- Commits GPG-signed (`-S`), authored `code kitten <yourcodekitten@gmail.com>`.

---

### Task 1: Pure card builder — fallback shape first

**Files:**
- Modify: `crates/fulfillment/src/whisper.rs` (replace `whisper_message` with `whisper_card`; port its two tests' assertions into card tests)

**Interfaces:**
- Produces: `pub enum PreviewKind { NewestDelivered, DryRunPick }` (derive Debug, Clone, Copy, PartialEq).
- Produces: `pub fn whisper_card(game: &Game, steam: Option<&dynamo::SteamAppCache>, site_url: &str, cycle: u32, slot: &str, preview: Option<PreviewKind>) -> serde_json::Value` — the FULL webhook body `{content, embeds, allowed_mentions}`. Preview marking lives in BOTH the content prefix and the footer (footer names the kind: `newest delivered` / `today's dry pick`).
- Produces: `pub(crate) fn trunc(s: &str, max: usize) -> String` — char-boundary-safe, `…`-suffixed.

- [x] **Step 1: Write the failing tests** (append to whisper.rs `mod tests`; delete `message_carries_title_bundle_deeplink_and_art` and `deeplink_urlencodes_the_title` in the same edit — their assertions move here)

```rust
#[test]
fn card_without_steam_carries_v1_information() {
    let g = game_with_bundle("g1", "Overgrowth", "Humble Indie Bundle 9", Some("https://art/x.png"));
    let v = whisper_card(&g, None, "https://bendobundles.com", 0, "2026-W36", None);
    let content = v["content"].as_str().unwrap();
    assert!(content.starts_with("🕯️"));
    assert!(content.contains("**Overgrowth**"));
    assert!(content.contains("Humble Indie Bundle 9"));
    assert!(content.contains("https://bendobundles.com/admin/catalog?q=Overgrowth"));
    assert!(!content.contains("https://art/x.png")); // art rides the embed now, never the content
    let embeds = v["embeds"].as_array().unwrap();
    assert_eq!(embeds.len(), 1);
    assert_eq!(embeds[0]["title"], "Overgrowth");
    assert_eq!(embeds[0]["image"]["url"], "https://art/x.png");
    assert_eq!(v["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
}

#[test]
fn card_deeplink_urlencodes_the_title() {
    let g = game_with_bundle("g1", "Papers, Please", "HB 12", None);
    let v = whisper_card(&g, None, "https://bendobundles.com", 0, "2026-W36", None);
    assert!(v["content"].as_str().unwrap().contains("catalog?q=Papers%2C%20Please"));
}

#[test]
fn card_preview_marks_content_and_footer_and_names_its_kind() {
    let g = game("g1", "aaa", None);
    let real = whisper_card(&g, None, "https://s", 0, "2026-W36", None);
    let prev = whisper_card(&g, None, "https://s", 0, "2026-W36", Some(PreviewKind::NewestDelivered));
    let dry = whisper_card(&g, None, "https://s", 0, "2026-W36", Some(PreviewKind::DryRunPick));
    assert!(prev["content"].as_str().unwrap().starts_with("🔍 *preview"));
    // the FOOTER is the mechanism — it travels with the embeds, the part anyone looks at
    assert!(prev["embeds"][0]["footer"]["text"].as_str().unwrap().starts_with("🔍 preview — newest delivered"));
    assert!(dry["embeds"][0]["footer"]["text"].as_str().unwrap().starts_with("🔍 preview — today's dry pick"));
    let strip = |v: &serde_json::Value| v["embeds"][0]["footer"]["text"].as_str().unwrap().rsplit("the attic whispers").next().unwrap().to_string();
    assert_eq!(strip(&real), strip(&prev)); // same card body under the marking
    assert!(!real["content"].as_str().unwrap().contains("preview"));
    assert!(!real["embeds"][0]["footer"]["text"].as_str().unwrap().contains("preview"));
}

#[test]
fn trunc_is_char_boundary_safe() {
    assert_eq!(trunc("abc", 5), "abc");
    assert_eq!(trunc("abcdef", 5), "abcd…");
    let s = "♡♡♡♡"; // 3-byte chars — a byte-index cut would panic
    assert_eq!(trunc(s, 3), "♡♡…");
}
```

- [x] **Step 2: Run to verify they fail**

Run: `cargo test -p fulfillment whisper -- --nocapture` (from repo root)
Expected: FAIL — `whisper_card` / `trunc` not found (and the two deleted tests gone).

- [x] **Step 3: Minimal implementation**

```rust
pub const EMBED_TITLE_MAX: usize = 256;
pub const EMBED_DESC_MAX: usize = 4096;
pub const EMBED_FIELD_VALUE_MAX: usize = 1024;
pub const EMBED_TOTAL_TEXT_MAX: usize = 6000;
pub const MAX_EMBEDS: usize = 10;
pub const GALLERY_GROUP: usize = 4;

/// Char-boundary-safe truncation with a `…` marker. `max` counts CHARS (Discord counts
/// characters, not bytes).
pub(crate) fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Which shape a 🔍 preview is showing — the footer names it, because the two render identically
/// otherwise (family round 1, Lilith ③).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    NewestDelivered,
    DryRunPick,
}

impl PreviewKind {
    fn label(self) -> &'static str {
        match self {
            PreviewKind::NewestDelivered => "newest delivered",
            PreviewKind::DryRunPick => "today's dry pick",
        }
    }
}

/// The full webhook body: the v1 voice in `content` (minus the bare art URL — the embed carries
/// art now) + the details-card embeds. Pure on purpose: every Discord-limit rule is pinned by a
/// test below and the handler just POSTs the value. Preview marking lives in the FOOTER first
/// (it travels with the embeds — the salient part) and the content prefix second.
pub fn whisper_card(
    game: &Game,
    steam: Option<&dynamo::SteamAppCache>,
    site_url: &str,
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
) -> serde_json::Value {
    let q = urlencoding::encode(&game.title);
    let mut content = format!(
        "🕯️ *from the attic…*\n**{title}** has been sleeping in *{bundle}*.\ncut a link for someone ♡ → {site}/admin/catalog?q={q}",
        title = game.title, bundle = game.bundle, site = site_url,
    );
    if let Some(k) = preview {
        content = format!("🔍 *preview — {}, not a new whisper*\n{content}", k.label());
    }
    let embeds = build_embeds(game, steam, cycle, slot, preview);
    serde_json::json!({
        "content": content,
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    })
}

/// The footer, one constructor: preview marking is a PREFIX (travels with the embeds), the
/// trimmed/trailer-cut markers SUFFIXES. Every embed-set builder ends by installing this on
/// embeds[0]. `trailer_cut` names the highest-priority loser when even the link cannot fit —
/// a budget that can't seat its highest-priority element needs a stated loser, not an accident.
fn footer_text(cycle: u32, slot: &str, preview: Option<PreviewKind>, trimmed: bool, trailer_cut: bool) -> String {
    let mut f = String::new();
    if let Some(k) = preview {
        f.push_str(&format!("🔍 preview — {} · ", k.label()));
    }
    f.push_str(&format!("the attic whispers · cycle {cycle} · {slot}"));
    if trimmed {
        f.push_str(" · trimmed to fit");
    }
    if trailer_cut {
        f.push_str(" · trailer link cut");
    }
    f
}

/// Task 1 shape: fallback embed only (steam handled in Task 2's extension of this fn).
fn build_embeds(
    game: &Game,
    _steam: Option<&dynamo::SteamAppCache>,
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
) -> Vec<serde_json::Value> {
    let title = trunc(&game.title, EMBED_TITLE_MAX);
    let trimmed = title != game.title;
    let mut e = serde_json::json!({
        "title": title,
        "footer": { "text": footer_text(cycle, slot, preview, trimmed, false) },
    });
    if let Some(art) = &game.artwork_url {
        e["image"] = serde_json::json!({ "url": art });
    }
    vec![e]
}
```

Also: delete `pub fn whisper_message` and fix `lib.rs`'s call site to compile — TEMPORARY shim in this task only, replaced for real in Task 3:

```rust
// lib.rs, in handle_whisper, replacing `let text = whisper::whisper_message(...)`:
let body = whisper::whisper_card(pick, None, &deps.whisper_site_url, cycle, &slot, None);
let text = body["content"].as_str().unwrap_or_default().to_string();
```

(The shim keeps v1 wire behavior — content-only — so this task is independently shippable; Task 3 swaps the send itself.)

- [x] **Step 4: Run to verify green**

Run: `cargo test -p fulfillment -- --nocapture` — all whisper unit tests + handler tests pass.
MEASURED at plan time (gate-5 MAJOR: an "if" is a judgement call a cold executor cannot resolve —
so it was resolved here): the seven whisper integration arms at `handler_test.rs:9118-9237`
(`whisper_envelope_parses` · `dark_param` · `unresolved_param` · `happy_path` · `send_failure` ·
`second_run_same_slot` · `empty_pool`) all build games via `available_game`, whose `artwork_url`
is `None` — **no arm asserts the art URL; NONE change in this task.** `happy_path` asserts
`contains("Overgrowth")` and `contains("from the attic")`, both preserved verbatim in the v2
content line. If any handler arm goes red here anyway, STOP and re-read it — do not silently
adjust an assertion this plan says should hold.

- [x] **Step 5: Commit**

```bash
git add crates/fulfillment/src/whisper.rs crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "feat(whisper): pure card builder — fallback shape, preview prefix, v1 voice preserved"
```

### Task 2: Card builder — full steam card, galleries, budgets

**Files:**
- Modify: `crates/fulfillment/src/whisper.rs` (extend `build_embeds`)

**Interfaces:**
- Consumes: `dynamo::SteamAppCache { detail: Option<steam_client::SteamAppDetail>, overall: Option<ReviewSummary>, recent: Option<RecentReviews>, .. }`
- Produces: unchanged `whisper_card` signature; `build_embeds` now renders the full card.

- [x] **Step 1: Write the failing tests**

```rust
// test helper: a full cache blob with n screenshots
fn steam_cache(n_shots: usize, hls: bool) -> dynamo::SteamAppCache {
    dynamo::SteamAppCache {
        app_id: 570,
        detail: Some(steam_client::SteamAppDetail {
            app_id: 570,
            name: "Overgrowth".into(),
            developers: vec!["Wolfire".into()],
            publishers: vec!["Wolfire".into()], // == devs → suppressed
            genres: vec!["Action".into()],
            release_date: Some("Oct 16, 2017".into()),
            short_description: "a rabbit does kung fu.".into(),
            header_image: Some("https://cdn/header.jpg".into()),
            video_hls_url: hls.then(|| "https://cdn/movie.m3u8".into()),
            video_thumbnail: Some("https://cdn/vthumb.jpg".into()),
            screenshots: (0..n_shots).map(|i| steam_client::Screenshot {
                thumbnail: format!("https://cdn/s{i}t.jpg"),
                full: format!("https://cdn/s{i}.jpg"),
            }).collect(),
            tags: vec!["Ninja".into(), "Rabbits".into()],
            content_descriptor_ids: vec![2, 5],
            content_notes: Some("cartoon rabbit violence".into()),
        }),
        overall: Some(steam_client::ReviewSummary {
            desc: "Very Positive".into(), total_positive: 900, total_negative: 100, total_reviews: 1000,
        }),
        recent: Some(steam_client::RecentReviews { percent_positive: 88, count: 42 }),
        fetched_at: 0, reviews_fetched_at: 0,
    }
}

#[test]
fn card_full_blob_renders_every_card_element() {
    let mut g = game("g1", "Overgrowth", Some("https://art/x.png"));
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&steam_cache(2, true)), "https://s", 3, "2026-W36", None);
    let e0 = &v["embeds"][0];
    assert_eq!(e0["url"], "https://store.steampowered.com/app/570");
    assert!(e0["description"].as_str().unwrap().contains("a rabbit does kung fu."));
    assert!(e0["description"].as_str().unwrap().contains("[🎬 watch the trailer](https://store.steampowered.com/app/570)"));
    let fields = e0["fields"].as_array().unwrap();
    let get = |n: &str| fields.iter().find(|f| f["name"] == n).unwrap()["value"].as_str().unwrap().to_string();
    assert_eq!(get("by"), "Wolfire");                       // pubs suppressed when == devs
    assert_eq!(get("released"), "Oct 16, 2017");
    assert_eq!(get("tags"), "Ninja · Rabbits");             // tags outrank genres, card rule
    assert_eq!(get("reviews"), "Very Positive — 90% of 1,000 (88% of 42 recent)");
    assert_eq!(get("bundle"), "Humble Test Bundle (steam)");
    // media completeness (family round 1): with screenshots present, embed[0].image is
    // screenshots[0] — the header consumes NO image slot; it rides the thumbnail chain
    assert_eq!(e0["image"]["url"], "https://cdn/s0.jpg");
    assert_eq!(e0["thumbnail"]["url"], "https://cdn/vthumb.jpg"); // video_thumbnail ?? header ?? artwork
    assert!(e0["footer"]["text"].as_str().unwrap().contains("cycle 3"));
    assert!(e0["footer"]["text"].as_str().unwrap().contains("2026-W36"));
    assert!(!e0["footer"]["text"].as_str().unwrap().contains("trimmed")); // nothing trimmed here
}

#[test]
fn card_without_screenshots_keeps_header_as_image() {
    let mut g = game("g1", "aaa", None);
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&steam_cache(0, false)), "https://s", 0, "2026-W36", None);
    assert_eq!(v["embeds"][0]["image"]["url"], "https://cdn/header.jpg"); // nothing displaced it
}

#[test]
fn card_never_leaks_admin_only_descriptors() {
    let mut g = game("g1", "aaa", None);
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&steam_cache(1, false)), "https://s", 0, "2026-W36", None);
    let s = serde_json::to_string(&v).unwrap();
    assert!(!s.contains("cartoon rabbit violence")); // #71: admin-only, spec's own exclusion
}

#[test]
fn card_ten_screenshots_all_ship_zero_silent_drops() {
    // family round 1, the blocking finding: the first layout promised header + 10 shots = 11
    // images in 10 slots and PINNED the silent drop of s9. This test is the anti-pin: ALL TEN.
    let mut g = game("g1", "aaa", None);
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&steam_cache(10, false)), "https://s", 0, "2026-W36", None);
    let embeds = v["embeds"].as_array().unwrap();
    assert_eq!(embeds.len(), MAX_EMBEDS); // main(s0) + 3 gallery-A(s1-3) + 4 B(s4-7) + 2 C(s8-9)
    let base = "https://store.steampowered.com/app/570";
    let urls: Vec<&str> = embeds.iter().map(|e| e["url"].as_str().unwrap()).collect();
    assert_eq!(urls.iter().filter(|u| **u == base).count(), 4);                       // group A
    assert_eq!(urls.iter().filter(|u| **u == format!("{base}#more")).count(), 4);     // group B
    assert_eq!(urls.iter().filter(|u| **u == format!("{base}#more2")).count(), 2);    // group C
    let images: Vec<&str> = embeds.iter().filter_map(|e| e["image"]["url"].as_str()).collect();
    for i in 0..10 {
        assert!(images.contains(&format!("https://cdn/s{i}.jpg").as_str()), "screenshot {i} missing");
    }
}

#[test]
fn card_two_screenshots_make_one_gallery_and_no_empty_groups() {
    let mut g = game("g1", "aaa", None);
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&steam_cache(2, false)), "https://s", 0, "2026-W36", None);
    let embeds = v["embeds"].as_array().unwrap();
    assert_eq!(embeds.len(), 2); // main carries s0; one gallery member carries s1; no #more groups
    assert!(embeds.iter().all(|e| !e["url"].as_str().unwrap().contains("#more")));
}

#[test]
fn card_truncation_announces_itself_in_the_footer() {
    let mut cache = steam_cache(0, false);
    if let Some(d) = cache.detail.as_mut() { d.short_description = "x".repeat(9000); }
    let mut g = game("g1", "aaa", None);
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&cache), "https://s", 0, "2026-W36", None);
    assert!(v["embeds"][0]["footer"]["text"].as_str().unwrap().ends_with("· trimmed to fit"));
}

#[test]
fn card_text_budget_holds_under_hostile_description() {
    let mut cache = steam_cache(10, true);
    if let Some(d) = cache.detail.as_mut() { d.short_description = "x".repeat(9000); }
    let mut g = game("g1", &"t".repeat(300), None);
    g.steam_app_id = Some(570);
    let v = whisper_card(&g, Some(&cache), "https://s", 0, "2026-W36", None);
    let embeds = v["embeds"].as_array().unwrap();
    assert_eq!(embeds[0]["title"].as_str().unwrap().chars().count(), EMBED_TITLE_MAX);
    // gate-5 blocker 2: the trailer link must SURVIVE the hostile description — it is reserved
    // before truncation, never the tail that gets cut
    assert!(embeds[0]["description"].as_str().unwrap().contains("[🎬 watch the trailer]"));
    assert!(embeds[0]["footer"]["text"].as_str().unwrap().contains("trimmed to fit"));
    let total: usize = embeds.iter().map(|e| {
        e["title"].as_str().unwrap_or("").chars().count()
            + e["description"].as_str().unwrap_or("").chars().count()
            + e["footer"]["text"].as_str().unwrap_or("").chars().count()
            + e["fields"].as_array().map(|fs| fs.iter().map(|f|
                f["name"].as_str().unwrap_or("").chars().count()
                + f["value"].as_str().unwrap_or("").chars().count()).sum::<usize>()).unwrap_or(0)
    }).sum();
    assert!(total <= EMBED_TOTAL_TEXT_MAX, "combined embed text {total} > {EMBED_TOTAL_TEXT_MAX}");
}

#[test]
fn card_degrades_per_half_when_reviews_or_detail_missing() {
    let mut g = game("g1", "aaa", Some("https://art/x.png"));
    g.steam_app_id = Some(570);
    let mut only_reviews = steam_cache(0, false);
    only_reviews.detail = None; // negative-cache stub
    let v = whisper_card(&g, Some(&only_reviews), "https://s", 0, "2026-W36", None);
    let fields = v["embeds"][0]["fields"].as_array().unwrap();
    assert!(fields.iter().any(|f| f["name"] == "reviews"));
    assert!(!fields.iter().any(|f| f["name"] == "by"));
    assert_eq!(v["embeds"][0]["image"]["url"], "https://art/x.png"); // artwork fallback
    let mut only_detail = steam_cache(0, false);
    only_detail.overall = None; only_detail.recent = None;
    let v2 = whisper_card(&g, Some(&only_detail), "https://s", 0, "2026-W36", None);
    assert!(!v2["embeds"][0]["fields"].as_array().unwrap().iter().any(|f| f["name"] == "reviews"));
}

#[test]
fn card_thumbnail_dropped_when_it_would_duplicate_image() {
    let mut g = game("g1", "aaa", None);
    g.steam_app_id = Some(570);
    let mut cache = steam_cache(0, false);
    if let Some(d) = cache.detail.as_mut() { d.video_thumbnail = None; d.header_image = None; }
    // image falls back to artwork; thumbnail would fall back to the same artwork → dropped
    let mut g2 = g.clone(); g2.artwork_url = Some("https://art/same.png".into());
    let v = whisper_card(&g2, Some(&cache), "https://s", 0, "2026-W36", None);
    assert_eq!(v["embeds"][0]["image"]["url"], "https://art/same.png");
    assert!(v["embeds"][0].get("thumbnail").is_none());
}
```

- [x] **Step 2: Run to verify the new tests fail** — `cargo test -p fulfillment whisper` → every test added in Step 1 FAILS (missing url/fields/galleries — count them in the output rather than trusting a number here), Task 1's stay green.

- [x] **Step 3: Implement — replace `build_embeds`**

```rust
fn build_embeds(
    game: &Game,
    steam: Option<&dynamo::SteamAppCache>,
    cycle: u32,
    slot: &str,
    preview: Option<PreviewKind>,
) -> Vec<serde_json::Value> {
    let detail = steam.and_then(|c| c.detail.as_ref());
    let store_url = game
        .steam_app_id
        .map(|id| format!("https://store.steampowered.com/app/{id}"));
    let mut trimmed = false; // any truncation flips this; the footer announces it

    let title = trunc(&game.title, EMBED_TITLE_MAX);
    trimmed |= title != game.title;
    let mut main = serde_json::json!({ "title": title });
    if let Some(u) = &store_url {
        main["url"] = serde_json::json!(u);
    }
    // footer + fields text participate in the 6000 budget; compute the fixed parts first and
    // give the description whatever remains. Worst-case footer (preview + trimmed) is budgeted so
    // flipping `trimmed` late can never overflow what was already measured.
    let footer_max = footer_text(cycle, slot, preview, true, true);

    let mut fields: Vec<(String, String)> = Vec::new();
    if let Some(d) = detail {
        let devs = d.developers.join(", ");
        let pubs = d.publishers.join(", ");
        let by = if pubs.is_empty() || pubs == devs { devs.clone() } else { format!("{devs} · {pubs}") };
        if !by.is_empty() { fields.push(("by".into(), by)); }
        if let Some(r) = &d.release_date { fields.push(("released".into(), r.clone())); }
        let tags: &[String] = if d.tags.is_empty() { &d.genres } else { &d.tags };
        if !tags.is_empty() { fields.push(("tags".into(), tags.join(" · "))); }
    }
    if let Some(c) = steam {
        let line = match (&c.overall, &c.recent) {
            (Some(o), r) => {
                let pct = ((o.total_positive as f64 / (o.total_reviews.max(1) as f64)) * 100.0).round() as u64;
                let total = fmt_thousands(o.total_reviews);
                let recent = r.as_ref().map(|r| format!(" ({}% of {} recent)", r.percent_positive, fmt_thousands(r.count))).unwrap_or_default();
                Some(format!("{} — {pct}% of {total}{recent}", o.desc))
            }
            (None, Some(r)) => Some(format!("{}% of {} recent", r.percent_positive, fmt_thousands(r.count))),
            (None, None) => None,
        };
        if let Some(l) = line { fields.push(("reviews".into(), l)); }
    }
    fields.push(("bundle".into(), format!("{} ({})", game.bundle, game.key_type)));

    let field_cap_hit = fields.iter().any(|(_, v)| v.chars().count() > EMBED_FIELD_VALUE_MAX);
    trimmed |= field_cap_hit;
    let fields_chars: usize = fields
        .iter()
        .map(|(n, v)| n.chars().count() + v.chars().count().min(EMBED_FIELD_VALUE_MAX))
        .sum();
    let fixed = title_len_after_trunc(&game.title) + footer_max.chars().count() + fields_chars;

    // TRAILER LINK SURVIVES TRUNCATION BY CONSTRUCTION (gate-5 blocker 2 + Lilith's edge): trunc
    // cuts the TAIL and the link was the tail — any near-budget description silently ate the
    // trailer. The loser order is STATED, not accidental: ① description tail loses to the link
    // (reserve the link's chars first, truncate only the body); ② if the link ITSELF cannot fit,
    // it is cut and the footer NAMES it — no unsigned subtraction anywhere near the boundary
    // (debug panic / release wrap-to-usize::MAX → 6000 blown → Discord 400 → whisper not sent).
    // NOTE: arm ② is unreachable through whisper_card's real inputs today (worst-case fixed text
    // ≈5.6k of 6000, link ≈60) — guarded anyway because the spec promises the build is TOTAL, and
    // commented as defensive rather than tested with a fixture no input can produce.
    let mut description = String::new();
    let mut trailer_cut = false;
    if let Some(d) = detail {
        let link = match (&d.video_hls_url, &store_url) {
            (Some(_), Some(u)) => format!("\n\n[🎬 watch the trailer]({u})"),
            _ => String::new(),
        };
        let total_budget = EMBED_DESC_MAX.min(EMBED_TOTAL_TEXT_MAX.saturating_sub(fixed));
        let link_len = link.chars().count();
        if link_len <= total_budget {
            let body = trunc(&d.short_description, total_budget - link_len); // guarded: link_len <= total_budget
            trimmed |= body != d.short_description;
            description = format!("{body}{link}");
        } else {
            trailer_cut = !link.is_empty();
            trimmed |= trailer_cut;
            description = trunc(&d.short_description, total_budget);
            trimmed |= description != d.short_description;
        }
    }
    if !description.is_empty() { main["description"] = serde_json::json!(description); }

    main["fields"] = serde_json::json!(fields.iter().map(|(n, v)| serde_json::json!({
        "name": trunc(n, EMBED_TITLE_MAX), "value": trunc(v, EMBED_FIELD_VALUE_MAX), "inline": true,
    })).collect::<Vec<_>>());

    // MEDIA COMPLETENESS (family round 1, the blocking finding): with screenshots present,
    // embed[0].image is screenshots[0] so the header consumes NO image slot — 10 shots fit in 10
    // embeds. Header art rides the thumbnail chain instead; with no screenshots it stays the image.
    let shots: &[steam_client::Screenshot] = detail.map(|d| d.screenshots.as_slice()).unwrap_or(&[]);
    let image = shots
        .first()
        .map(|s| s.full.clone())
        .or_else(|| detail.and_then(|d| d.header_image.clone()))
        .or_else(|| game.artwork_url.clone());
    if let Some(img) = &image { main["image"] = serde_json::json!({ "url": img }); }
    let thumb = detail
        .and_then(|d| d.video_thumbnail.clone())
        .or_else(|| detail.and_then(|d| d.header_image.clone()))
        .or_else(|| game.artwork_url.clone());
    if let (Some(t), true) = (&thumb, thumb != image) {
        main["thumbnail"] = serde_json::json!({ "url": t });
    }

    main["footer"] = serde_json::json!({ "text": footer_text(cycle, slot, preview, trimmed, trailer_cut) });

    let mut embeds = vec![main];
    // galleries carry screenshots[1..] (screenshots[0] is embed[0]'s image). Grouping is keyed on
    // `url` — client rendering, not API contract; if it ever stops, this degrades to a tall
    // column, nothing lost. Nothing may ASSUME three groups.
    if let Some(base) = &store_url {
        for (i, shot) in shots.iter().enumerate().skip(1) {
            if embeds.len() >= MAX_EMBEDS { break; }
            let group = i / GALLERY_GROUP; // i 1-3 ride group A with the main embed; 4-7 B; 8-9 C
            let url = match group {
                0 => base.clone(),
                1 => format!("{base}#more"),
                _ => format!("{base}#more2"),
            };
            embeds.push(serde_json::json!({ "url": url, "image": { "url": shot.full } }));
        }
    }
    embeds
}

/// Title length after the trunc pass — small helper so `fixed` and the emitted title agree.
fn title_len_after_trunc(t: &str) -> usize {
    trunc(t, EMBED_TITLE_MAX).chars().count()
}

/// 1,234-style thousands formatting (the card uses toLocaleString; Discord gets the same shape).
fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 { out.push(','); }
        out.push(ch);
    }
    out
}
```

- [x] **Step 4: Run to verify green** — `cargo test -p fulfillment whisper` → all pass. Also `cargo clippy -p fulfillment --all-targets`.

- [x] **Step 5: Commit**

```bash
git add crates/fulfillment/src/whisper.rs
git commit -S -m "feat(whisper): full details card — fields, review math, 3-group screenshot galleries, hostile-input budgets"
```

### Task 3: JSON-body deliver twin + wire the card into handle_whisper

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (`deliver_json`, `whisper_send_body`, `handle_whisper` swap, remove Task 1's shim)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `whisper::whisper_card` (Task 1/2 signature).
- Produces: `async fn deliver_json(http: &reqwest::Client, url: &str, body: &serde_json::Value) -> u32` (1 = failure, 0 = success — `deliver`'s exact polarity, incl. the load-bearing `Ok(non-2xx) → 1` arm); `async fn whisper_send_body(http, url, body) -> bool` (`deliver_json(..) == 0`); `#[doc(hidden)] pub async fn whisper_send_body_for_test(url: &str, body: &serde_json::Value) -> bool` (the test seam, consumed by this task's own 429 arm and nothing else).

- [x] **Step 1: Write the failing integration tests** (handler_test.rs, next to the existing whisper arms; reuse `deps_whisper` + the discord MockServer)

```rust
/// Integration-side steam cache fixture (minimal: detail half only — the unit tests own the full
/// per-field rendering; these arms prove the WIRE). Seeded via put_steam_app Absent guard.
fn seeded_cache(app_id: u32) -> dynamo::SteamAppCache {
    dynamo::SteamAppCache {
        app_id,
        detail: Some(steam_client::SteamAppDetail {
            app_id,
            name: "Overgrowth".into(),
            developers: vec!["Wolfire".into()],
            publishers: vec![],
            genres: vec!["Action".into()],
            release_date: Some("Oct 16, 2017".into()),
            short_description: "a rabbit does kung fu.".into(),
            header_image: Some("https://cdn/header.jpg".into()),
            video_hls_url: None,
            video_thumbnail: None,
            screenshots: vec![],
            tags: vec![],
            content_descriptor_ids: vec![],
            content_notes: None,
        }),
        overall: None,
        recent: None,
        fetched_at: 0,
        reviews_fetched_at: 0,
    }
}

#[tokio::test]
async fn whisper_posts_embed_card_with_mention_denial() {
    let Some(store) = store_or_skip("whisper-card").await else { return; };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&wh).await;
    let mut g = available_game("g1", "Overgrowth");
    g.steam_app_id = Some(570);
    store.put_game(&g).await.unwrap();
    store.put_steam_app(&seeded_cache(570), dynamo::SteamAppPutGuard::Absent).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    handle(&d, FulfillRequest::Whisper).await;
    let reqs = wh.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert!(!body["embeds"].as_array().unwrap().is_empty()); // the card is ON THE WIRE
    assert_eq!(body["embeds"][0]["title"], "Overgrowth");
    assert!(body["embeds"][0]["description"].as_str().unwrap().contains("a rabbit does kung fu."));
    assert_eq!(body["allowed_mentions"]["parse"].as_array().unwrap().len(), 0); // #174, on the new shape
    assert!(body["content"].as_str().unwrap().starts_with("🕯️"));
}

#[tokio::test]
async fn whisper_steam_cache_miss_degrades_to_fallback_card_still_delivered() {
    let Some(store) = store_or_skip("whisper-card-nocache").await else { return; };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&wh).await;
    let mut g = available_game("g1", "Aaa");
    g.steam_app_id = Some(999_999); // no cache row exists for this app id
    store.put_game(&g).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    handle(&d, FulfillRequest::Whisper).await;
    let ws = store.list_whispers().await.unwrap();
    assert_eq!(ws.len(), 1);
    assert!(ws[0].delivered); // cache is best-effort: the whisper still went
    let body: serde_json::Value =
        serde_json::from_slice(&wh.received_requests().await.unwrap()[0].body).unwrap();
    assert_eq!(body["embeds"].as_array().unwrap().len(), 1); // fallback card, no phantom galleries
}

#[tokio::test]
async fn whisper_send_body_treats_non_2xx_as_failure() {
    // mirrors ping_treats_non_2xx_as_failure for the JSON twin: a 429 must not read as sent
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(429)).expect(1).mount(&wh).await;
    let ok = fulfillment::whisper_send_body_for_test(&wh.uri(), &serde_json::json!({"content": "x"})).await;
    assert!(!ok, "a 429 landed in the success branch — the Ok(non-2xx) arm is missing");
}
```

(Imports this task adds to handler_test.rs: `dynamo::SteamAppPutGuard` — everything else is already
in scope. `store_or_skip`, `deps_whisper`, `available_game`, `handle` are the file's own fixtures.)

(Write them as real tests against the existing harness — the exact seeding calls are visible in the
neighboring whisper arms in this same file; follow their shape. The assertions above are the
contract. Harness idiom, concretely: `let discord = MockServer::start().await;` +
`Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).mount(&discord).await;`,
build deps via the file's `deps_whisper(...)` fixture pointing `whisper_notify` at
`discord.uri()`, invoke through the same handler entry the neighboring arms use, then read the
captured payload with `discord.received_requests().await.unwrap()` and
`serde_json::from_slice::<serde_json::Value>(&reqs[0].body)`. For the 429 arm, respond with
`ResponseTemplate::new(429)` and call the `whisper_send_body_for_test` seam directly.)

- [x] **Step 2: Run to verify they fail** — `cargo test -p fulfillment --test handler_test whisper`: `whisper_send_body_treats_non_2xx_as_failure` fails to COMPILE (`whisper_send_body_for_test` not found — the right red for a missing seam); comment it out for one run and the two wire arms fail on ASSERTION (`body["embeds"]` empty/absent — v1 still sends content-only after Task 1's shim). Both failure modes stated so neither surprises.

- [x] **Step 3: Implement**

```rust
/// POST one prebuilt JSON body. Same polarity and the same load-bearing Ok(non-2xx)→failure arm
/// as [`deliver`] — a 429 must not read as sent. The body is expected to carry its own
/// allowed_mentions (whisper_card always does; pinned there).
async fn deliver_json(http: &reqwest::Client, url: &str, body: &serde_json::Value) -> u32 {
    match http.post(url).json(body).send().await {
        Ok(r) if r.status().is_success() => 0,
        Ok(r) => {
            tracing::error!(status = %r.status(), "whisper card POST non-success");
            1
        }
        Err(e) => {
            tracing::error!(error = %e, "whisper card POST transport failure");
            1
        }
    }
}

async fn whisper_send_body(http: &reqwest::Client, url: &str, body: &serde_json::Value) -> bool {
    deliver_json(http, url, body).await == 0
}

#[doc(hidden)]
pub async fn whisper_send_body_for_test(url: &str, body: &serde_json::Value) -> bool {
    whisper_send_body(&reqwest::Client::new(), url, body).await
}
```

In `handle_whisper`, replace the shim + send:

```rust
    let steam = match pick.steam_app_id {
        None => None,
        // cache is best-effort exactly as public-api treats it: read failure degrades to the
        // fallback card, never blocks the whisper
        Some(app_id) => deps.store.get_steam_app(app_id).await.ok().flatten(),
    };
    let body = whisper::whisper_card(pick, steam.as_ref(), &deps.whisper_site_url, cycle, &slot, None);
    if whisper_send_body(&deps.http, &whisper_url, &body).await {
```

(the `mark_whisper_delivered` / cause-④ tail is unchanged; `whisper_send` becomes dead — delete it and its doc block.)

- [x] **Step 4: Run the full crate** — `cargo test -p fulfillment` (moto on :8000). All green, including every pre-existing whisper arm (they assert content substrings, preserved).

- [x] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "feat(whisper): send the card — deliver_json twin (non-2xx = failure), cache read degrades, v1 text path retired"
```

### Task 4: The `whisper_preview` envelope — zero writes

**Files:**
- Modify: `crates/fulfillment/src/lib.rs` (enum variant, dispatch, `resolve_whisper_url` extraction, `handle_whisper_preview`)
- Test: `crates/fulfillment/tests/handler_test.rs`

**Interfaces:**
- Consumes: `whisper_card(.., preview: Some(PreviewKind::...))`, `whisper_send_body`, `Store::{list_whispers, get_game, get_steam_app}`.
- Produces: `FulfillRequest::WhisperPreview` (wire: `{"op":"whisper_preview"}`) and THREE new `FulfillResponse` variants (family round 1, Lilith's ③ — a binary response has nowhere to put "the gate refused"): `PreviewSent` / `PreviewBlocked` / `PreviewSendFailed`, wire `{"result":"preview_sent"|"preview_blocked"|"preview_send_failed"}`. Ben invokes by hand; the lambda response JSON is where the verdict lands. `PreviewBlocked` covers dark ①a/①b, unreadable log/game/pool, and empty pool — each cause still emits its own distinct ping/log naming why; the response says only that no preview was sent.

- [x] **Step 1: Write the failing tests**

```rust
/// Order-independent whisper-log fingerprint — list order is not contractual, field equality is.
fn whisper_log_key(ws: &[domain::WhisperRecord]) -> Vec<(String, String, u32, bool)> {
    let mut k: Vec<_> = ws
        .iter()
        .map(|w| (w.slot.clone(), w.game_id.clone(), w.cycle, w.delivered))
        .collect();
    k.sort();
    k
}

#[tokio::test]
async fn whisper_preview_resends_newest_delivered_and_writes_nothing() {
    let Some(store) = store_or_skip("whisper-preview").await else { return; };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&wh).await;
    store.put_game(&available_game("g-old", "Older")).await.unwrap();
    store.put_game(&available_game("g-new", "Newer")).await.unwrap();
    assert!(store.record_whisper("2026-W34", "g-old", 0).await.unwrap());
    store.mark_whisper_delivered("2026-W34").await.unwrap();
    assert!(store.record_whisper("2026-W35", "g-new", 0).await.unwrap());
    store.mark_whisper_delivered("2026-W35").await.unwrap();
    let before = whisper_log_key(&store.list_whispers().await.unwrap());
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    let r = handle(&d, FulfillRequest::WhisperPreview).await;
    assert!(matches!(r, FulfillResponse::PreviewSent));
    let body: serde_json::Value =
        serde_json::from_slice(&wh.received_requests().await.unwrap()[0].body).unwrap();
    assert!(body["content"].as_str().unwrap().starts_with("🔍 *preview"));
    // the footer is the mechanism — it travels with the embeds, and it names WHICH shape
    assert!(body["embeds"][0]["footer"]["text"].as_str().unwrap().starts_with("🔍 preview — newest delivered"));
    // newest delivered = lexicographic max slot (slots are zero-padded ⇒ lex max IS chronological)
    assert_eq!(body["embeds"][0]["title"], "Newer");
    let after = whisper_log_key(&store.list_whispers().await.unwrap());
    assert_eq!(before, after); // ZERO WRITES — field-identical log
}

#[tokio::test]
async fn whisper_preview_with_empty_log_previews_todays_pick_without_recording() {
    let Some(store) = store_or_skip("whisper-preview-dry").await else { return; };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&wh).await;
    store.put_game(&available_game("g1", "Aaa")).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    let r = handle(&d, FulfillRequest::WhisperPreview).await;
    assert!(matches!(r, FulfillResponse::PreviewSent));
    let body: serde_json::Value =
        serde_json::from_slice(&wh.received_requests().await.unwrap()[0].body).unwrap();
    assert!(body["embeds"][0]["footer"]["text"].as_str().unwrap().starts_with("🔍 preview — today's dry pick"));
    assert!(store.list_whispers().await.unwrap().is_empty()); // the dry-run recorded NOTHING
}

#[tokio::test]
async fn whisper_preview_dark_says_blocked_not_whispered() {
    let Some(store) = store_or_skip("whisper-preview-dark").await else { return; };
    let ops = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(204)).expect(1).mount(&ops).await;
    store.put_game(&available_game("g1", "Aaa")).await.unwrap();
    let d = deps_whisper(store.clone(), &ops.uri(), Some(ops.uri()), None); // whisper DARK, ops on
    let r = handle(&d, FulfillRequest::WhisperPreview).await;
    // Lilith's ③: silence must not be byte-identical to sent-fine — the response says BLOCKED
    assert!(matches!(r, FulfillResponse::PreviewBlocked));
    assert!(store.list_whispers().await.unwrap().is_empty()); // zero writes on the dark path
}

#[tokio::test]
async fn whisper_preview_dead_webhook_says_send_failed() {
    let Some(store) = store_or_skip("whisper-preview-500").await else { return; };
    let wh = MockServer::start().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(500)).expect(1).mount(&wh).await;
    store.put_game(&available_game("g1", "Aaa")).await.unwrap();
    let d = deps_whisper(store.clone(), &wh.uri(), None, Some(wh.uri()));
    let r = handle(&d, FulfillRequest::WhisperPreview).await;
    assert!(matches!(r, FulfillResponse::PreviewSendFailed));
    assert!(store.list_whispers().await.unwrap().is_empty()); // failure still writes nothing
}

#[test]
fn whisper_preview_op_deserializes() {
    let r: fulfillment::FulfillRequest = serde_json::from_str(r#"{"op":"whisper_preview"}"#).unwrap();
    assert_eq!(r, fulfillment::FulfillRequest::WhisperPreview);
}
```

- [x] **Step 2: Run to verify they fail** — the four integration arms fail to COMPILE (`FulfillRequest::WhisperPreview`, `FulfillResponse::PreviewSent/Blocked/SendFailed` are unresolved paths — the right red for missing variants); `whisper_preview_op_deserializes` additionally fails at runtime on the unknown variant once the enum exists but before dispatch does.

- [x] **Step 3: Implement**

Enum + dispatch:

```rust
    /// Zero-write preview (spec v2): re-render + re-send the newest DELIVERED whisper's card
    /// (fallback: today's would-be pick, still without recording), marked 🔍 in footer + content,
    /// so card changes are visible without spending a weekly slot. Manual-invoke-only, like
    /// ValidateCookie.
    WhisperPreview,
...
        FulfillRequest::WhisperPreview => handle_whisper_preview(deps).await,
```

And in `FulfillResponse` (three-valued — the response is the verdict surface for a hand-invoked op):

```rust
    /// whisper_preview: the card went out. Which shape it showed is in the card's own footer.
    PreviewSent,
    /// whisper_preview: no preview was possible — dark webhook (either face), unreadable
    /// log/game/pool, or empty pool. Each cause announces itself in its own ping/log; this
    /// variant exists so silence is never byte-identical to sent-fine.
    PreviewBlocked,
    /// whisper_preview: the card was built and the POST failed.
    PreviewSendFailed,
```

Extract the dark-gate from `handle_whisper` verbatim into:

```rust
/// The whisper dark-gate, shared by whisper + preview: Some(url) ⟺ sendable; both dark faces
/// announce themselves distinctly and return None (①a advises the put-parameter; ①b never
/// advises overwrite). Extracted UNCHANGED from handle_whisper — the two faces' wording is
/// family-reviewed, do not edit in passing.
async fn resolve_whisper_url(deps: &Deps) -> Option<String> { /* moved match, verbatim */ }
```

Handler:

```rust
/// {"op":"whisper_preview"} — ZERO WRITES BY CONSTRUCTION: no record, no mark, no cycle movement;
/// the only side effect is one webhook POST (plus the dark-gate ops ping). The card is rendered by
/// the same whisper_card the real path uses — a preview that renders through different code
/// previews nothing.
async fn handle_whisper_preview(deps: &Deps) -> FulfillResponse {
    let Some(url) = resolve_whisper_url(deps).await else {
        return FulfillResponse::PreviewBlocked; // dark faces already pinged their own reason
    };
    let whispers = match deps.store.list_whispers().await {
        Ok(w) => w,
        Err(e) => {
            tracing::error!(error = ?e, "whisper_preview: cannot list whisper log — blocked");
            return FulfillResponse::PreviewBlocked;
        }
    };
    let newest = whispers.iter().filter(|w| w.delivered).max_by(|a, b| a.slot.cmp(&b.slot));
    let (game, cycle, slot, kind) = match newest {
        Some(w) => match deps.store.get_game(&w.game_id).await {
            Ok(Some(g)) => (g, w.cycle, w.slot.clone(), whisper::PreviewKind::NewestDelivered),
            _ => {
                tracing::error!(game = %w.game_id, "whisper_preview: delivered game unreadable — blocked");
                return FulfillResponse::PreviewBlocked;
            }
        },
        None => {
            // empty log: dry-run today's pick — same reads as the real path, NO record
            let (games, links) = match (deps.store.list_listable_games().await, deps.store.list_links().await) {
                (Ok(g), Ok(l)) => (g, l),
                _ => {
                    tracing::error!("whisper_preview: cannot read pool — blocked");
                    return FulfillResponse::PreviewBlocked;
                }
            };
            let now = OffsetDateTime::now_utc();
            let promises = whisper::active_promises(&links, now);
            let pool = whisper::eligible(&games, &promises, &std::collections::HashSet::new());
            let Some(pick) = whisper::select(&pool, i64::from(now.date().to_julian_day())) else {
                tracing::warn!("whisper_preview: empty pool — blocked, nothing to preview");
                return FulfillResponse::PreviewBlocked;
            };
            let (y, w, _) = now.date().to_iso_week_date();
            (pick.clone(), 0, format!("{y}-W{w:02}"), whisper::PreviewKind::DryRunPick)
        }
    };
    let steam = match game.steam_app_id {
        None => None,
        Some(app_id) => deps.store.get_steam_app(app_id).await.ok().flatten(),
    };
    let body = whisper::whisper_card(&game, steam.as_ref(), &deps.whisper_site_url, cycle, &slot, Some(kind));
    if whisper_send_body(&deps.http, &url, &body).await {
        FulfillResponse::PreviewSent
    } else {
        tracing::error!(outcome = "whisper_preview_send_failed", "preview POST failed");
        FulfillResponse::PreviewSendFailed
    }
}
```

- [x] **Step 4: Run** — `cargo test -p fulfillment` all green.

- [x] **Step 5: Commit**

```bash
git add crates/fulfillment/src/lib.rs crates/fulfillment/tests/handler_test.rs
git commit -S -m "feat(whisper): whisper_preview envelope — zero writes, newest-delivered or dry-run pick, shared dark-gate"
```

### Task 5: Spec status, workspace green, fmt/clippy

**Files:**
- Modify: `docs/spec-whisper-details-card.md` (status → implemented; resolve open questions with family answers), `docs/spec-attic-whispers.md` (one line: payload superseded by v2 spec, selection unchanged)

- [x] **Step 1:** Update both spec headers/notes as above.
- [x] **Step 2:** `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings`
- [x] **Step 3:** Full suite: `cargo test --workspace` (moto on :8000; 29 suites was the 08-28 baseline).
- [x] **Step 4:** Commit:

```bash
git add docs/
git commit -S -m "docs(whisper): v2 spec status + v1 payload note"
```

## Self-Review (run at plan time — done 2026-08-31)

- Spec coverage: card fields ✔ (T2), media/galleries ✔ (T2), fallbacks ✔ (T1/T2/T3), mention denial ✔ (T1 + wire test T3), preview ✔ (T4), no-infra ✔ (no task touches terraform), admin-descriptor exclusion ✔ (T2 negative test). Gap: none found.
- Placeholder scan: Task 3 Step 1 describes assertions in comments — deliberate: the seeding calls must copy the NEIGHBORING whisper arms' shape, which the executor can see; contract is stated. Everything else has real code.
- Type consistency: `whisper_card` signature identical in T1/T2/T3/T4; `deliver_json` polarity stated twice identically; `SteamAppCache` fields match `dynamo` source (verified in measurement).
- Open questions: T2's gallery count and trailer link + T4's channel follow my spec leans; if family answers differ (window still open), the affected constants/assertions change in T2/T4 only.
