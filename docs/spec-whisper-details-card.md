# spec — whisper v2: the full details card 💌🖼️

*2026-08-31 · kitten · status: draft, measuring done, open questions → family*

## the ask (verbatim, Ben, 2026-08-28)

> "Give me the full details of the game along with all of the media. I want everything that is on
> the details card for the game that a user would see."

Judged baseline: the first whisper (*Bleed 2*, plain-text form) — "Looks good. But…". The "but" is
this spec.

## what a USER actually sees (measured from `GameDetailModal.tsx` @ e87aad7, friend mount)

The friend details card renders, in order:

| element | source field | notes |
|---|---|---|
| media stage + contact sheet | `detail.video_hls_url` + `detail.screenshots[]` (≤10) | trailer first when present, then screenshots |
| header art fallback | `detail.header_image` ?? `game.artwork_url` | |
| title | `game.title` | |
| devs · pubs · date | `detail.developers/publishers/release_date` | pubs suppressed when == devs |
| steam ↗ link | `game.steam_app_id` | store.steampowered.com/app/{id} |
| genre/tag chips | `displayTags(detail)` = `tags` falling back to `genres` | |
| short description | `detail.short_description` | |
| review meter | `recent.percent_positive`+`count`, `overall` % + `total_reviews` + `desc` | spectrum-colored descriptor |
| bundle + key_type chips | `game.bundle`, `game.key_type` | (no-steam branch; bundle also in v1 whisper voice) |

**Deliberately excluded:** `content_descriptor_ids`/`content_notes` — admin-only on the card
(#71). Ben said *"that a user would see"*; a friend never sees these. Excluded BY THE SPEC'S OWN
CRITERION, not forgotten.

**Server-side availability (measured):** everything above lives in the dynamo steam cache —
`Store::get_steam_app(app_id) -> SteamAppCache { detail, overall, recent }` — cache-only, Steam
never called at request time (public-api `handle_game_detail` proves the pattern). The fulfillment
lambda already owns this table (its sync/enrichment pass WRITES the cache), so reads need zero IAM
change. `Deps.store` is the concrete dynamo `Store`; `get_steam_app` is directly callable.

## the vehicle — Discord webhook embeds

v1 posts `{"content": text, "allowed_mentions": {"parse": []}}`. v2 posts `content` (the whisper
voice line + deep-link stays text, so nothing about the register changes) **plus `embeds`**.

Discord constraints that shape the design (limits are Discord API constants):
- ≤10 embeds per message; **combined text** across embeds (title+description+fields+footer+author)
  ≤6000 chars. Per-embed: title ≤256, description ≤4096, field name ≤256 / value ≤1024.
- One embed renders ONE large `image`. **Gallery trick** (stable, documented Discord behavior):
  up to 4 embeds sharing an identical non-empty `url` merge their `image`s into one 4-image
  gallery under the first embed of the group. Distinct `url` values start distinct groups.
- A webhook embed cannot carry a playable video. The trailer must travel as a link.

### the card, as embeds — REVISED after family round 1 (Lilith's ① was blocking: the original
### layout promised header + 10 screenshots = 11 images in 10 slots, and its own test pinned the
### silent drop of the 10th. The header is the most droppable image — it moves to the thumbnail.

```
content: v1's whisper voice verbatim, minus the bare art URL (the embed carries art now)

embed[0]  (group A url = steam store page; url omitted when no steam app):
  title:        {game.title}                                  (≤256, truncated defensively)
  url:          https://store.steampowered.com/app/{app_id}
  description:  {short_description}
                + "\n\n🎬 [watch the trailer](store page)"    (only when video_hls_url present;
                  copy promises nothing — age-gated titles show a gate, never write "autoplays")
  fields (inline):
    "by"       → developers · publishers (pubs suppressed when == devs)
    "released" → release_date
    "tags"     → tags ?? genres, joined " · "
    "reviews"  → "{overall.desc} — {pct}% of {total} ({recent}% of {n} recent)"
    "bundle"   → {game.bundle} ({key_type})
  image:        screenshots[0].full  — when screenshots exist
                header_image ?? artwork_url — when none (nothing displaced)
  thumbnail:    video_thumbnail ?? header_image ?? artwork_url  (dropped when == image)
  footer:       "the attic whispers · cycle {cycle} · {slot}"
                (+ "🔍 preview — newest delivered|today's dry pick · " PREFIX in preview mode —
                 the marking travels with the salient part, not only in content)
                (+ " · trimmed to fit" SUFFIX when ANY truncation fired — a drop must announce
                 itself in production, not only in tests)

embeds[1..4]  (group A, same url):        image = screenshots[1..4]  → 4-image gallery w/ embed[0]
embeds[4..8]  (group B, url + "#more"):   image = screenshots[4..8]  → second gallery
embeds[8..10] (group C, url + "#more2"):  image = screenshots[8..10] → third gallery (partial)
```

10 embeds ⇒ **all 10 screenshots** (the card's own cap) — "all of the media" is literally
satisfied, zero silent drops. Groups only exist when they have images; a game with 2 screenshots
gets one small gallery; a game with none gets the single embed with header art.

**Provenance, named honestly (OMBB, round 1):**
- `allowed_mentions: {"parse": []}` is DOCUMENTED as covering message *content*. Embeds are never
  named by the docs; what protects the steam-wire text in embeds is that **embeds do not render
  mentions at all** — **observed behaviour, UNCITED** (neither account in the family round had a
  citation; agreement between reviewers is not a source). Two layers, each covering its own half;
  moving text across the content/embed line changes which guarantee applies. Both halves stay
  pinned by tests on the payload we send (Discord's behavior is not ours to test; the field is).
- The 4-per-gallery `url`-grouping is **client rendering, not API contract** (the 10-embed cap IS
  documented). **The failure mode, written down instead of the trick:** if grouping ever stops,
  Discord renders 10 stacked single-image embeds — a long scroll, no error, nothing lost.
  Survivable exactly as long as nothing downstream ASSUMES three groups — do not build anything
  that counts galleries.

### fallbacks — delight never gates (PRODUCT principle 5, same as v1 select())

- `steam_app_id == None` **or** cache miss **or** `cache.detail == None` (negative stub):
  the embed still ships — title, bundle field, artwork image, deep-link content line. That is v1's
  information content in v2's clothes, never a regression.
- `overall == None` → reviews field shows recent only, or is omitted entirely.
- Embed build is TOTAL: every string truncated to its Discord limit at the seam, the 6000-char
  combined budget enforced by dropping fields/description tail loudly in tests, never panicking.

### what does NOT change

- Selection predicate, cycle/rollover, RECORD → SEND → MARK, slot key, all five no-send causes —
  untouched. This spec is about the PAYLOAD only.
- `allowed_mentions: {"parse": []}` — **load-bearing (#174), stays on the new payload shape.**
  The embed body carries wire-derived text (title, description from Steam); the mention-power
  denial must cover it exactly as it covers content.
- The whisper voice: lowercase, ♡, friend register. The embed is the card; the content line is
  still the whisper. **Audience vs voice, disambiguated on purpose (OMBB, round 1): the whisper
  goes to BEN'S channel; "friend-surface" names the VOICE, never the audience.** No friend reads
  this room — don't get cautious about the wrong thing later.

## the preview envelope — `{"op":"whisper_preview"}` (Ben's parked idea, riding along)

A no-write sibling: re-render and re-send the card for **the most recent delivered whisper**
(fall back: the pick select() would make today, clearly marked), so Ben can see card changes
without spending a weekly slot.

- ZERO writes: no record, no mark, no cycle movement. Reuses the dark-gate (both faces) verbatim
  via an extracted `resolve_whisper_url` — the family-reviewed wording moves, unedited.
- Sends to the whisper webhook (it previews the whisper channel's rendering — an ops-channel
  preview would render under different channel settings and prove nothing). Marked 🔍 in BOTH the
  content prefix AND the footer (the footer is the mechanism — it travels with the embeds, the
  part anyone actually looks at), and the marking names WHICH shape is being previewed:
  `newest delivered` vs `today's dry pick` (they render identically otherwise; one word fixes it).
- **Three-valued response, not `Whispered`** (Lilith's ③: a binary response has nowhere to put
  "the gate refused" — dark-preview silence would be byte-identical to sent-fine and to 500'd):
  `preview_sent` · `preview_blocked` (dark ①a/①b, unreadable log/game, empty pool — each face
  still emits its own distinct ping/log naming why) · `preview_send_failed`. Ben invokes by hand,
  so the lambda response JSON is exactly where the verdict lands.
- Invocable only like every other envelope: by whoever holds lambda:InvokeFunction on fulfillment
  (Ben's CLI / the two API lambdas' IAM). No schedule, no new infra.

## implementation shape (for the plan)

1. `whisper.rs` (pure, I/O-free, where the family review teeth live):
   `whisper_card(game, steam: Option<&SteamAppCache>, site_url, cycle, slot) -> serde_json::Value`
   returning the FULL webhook body (content + embeds + allowed_mentions). Property tests pin:
   field truncation at every Discord limit, 6000-budget enforcement, gallery url-grouping (≤4 per
   group, group exists ⟺ images exist), no-steam fallback carries v1's information, mention
   denial present on every shape.
2. `lib.rs`: `handle_whisper` gains one read (`get_steam_app` when `steam_app_id.is_some()`;
   read failure degrades to the no-steam card — cache is best-effort exactly as public-api treats
   it) and calls `whisper_send_body` (POST a prebuilt JSON body; same rc-counting polarity as
   `deliver`, pinned by test). `handle_whisper_preview` = dark-gate → newest delivered whisper →
   load game+cache → card → send with preview prefix. New `FulfillRequest::WhisperPreview` +
   `FulfillResponse` reuse of `Whispered`.
3. No terraform, no IAM, no schedule change. Deploy = the #210 runbook (CI-built zips).

## open questions — ALL RESOLVED, family round 1 (2026-08-31 morning, shared channel)

1. **Media coverage: ALL 10 — and the original layout couldn't deliver it.** Lilith (blocking):
   header + 10 screenshots = 11 images in 10 slots; my own plan test had pinned the silent drop of
   the 10th. Fix adopted whole: embed[0] carries screenshots[0], header moves to the thumbnail
   chain. 10/10, zero silent drops.
2. **Trailer: store-page link, settled by measurement** — OMBB grepped the captured payload
   (`docs/superpowers/specs/captures/2026-07-06-steam/appdetails-413150-trimmed.json` movies[0]:
   dash_av1/dash_h264/hls_h264 only) + my own July note: Steam is HLS/DASH-only now, no mp4 to
   post. Copy promises nothing ("watch the trailer", never "autoplays" — age gates exist). Never
   pattern-derive an mp4 from the .m3u8: a guessed URL that resolves is the worst kind of working.
3. **Preview: same channel, marked in the footer, three-valued response** — see the preview
   section. Plus Lilith's unasked fourth: any truncation announces itself in the footer
   (" · trimmed to fit"), so the 6000-budget can never silently eat the review meter.
