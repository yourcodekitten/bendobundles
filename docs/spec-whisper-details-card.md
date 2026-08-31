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

### the card, as embeds

```
content: "🕯️ *from the attic…* — cut a link for someone ♡ → {site}/admin/catalog?q={title}"

embed[0]  (group A url = steam store page, or site deep-link when no steam app):
  title:        {game.title}                                  (≤256, truncated defensively)
  url:          https://store.steampowered.com/app/{app_id}
  description:  {short_description}
                + "\n\n🎬 [watch the trailer](store page)"    (only when video_hls_url present)
  fields (inline):
    "by"       → developers · publishers (pubs suppressed when == devs)
    "released" → release_date
    "tags"     → tags ?? genres, joined " · "
    "reviews"  → "{overall.desc} — {pct}% of {total} ({recent}% of {n} recent)"
    "bundle"   → {game.bundle} ({key_type})
  image:        header_image ?? artwork_url
  thumbnail:    video_thumbnail ?? artwork_url  (small corner art; skipped if == image)
  footer:       "the attic whispers · cycle {cycle} · {slot}"

embeds[1..3]  (group A, same url): image = screenshots[0..3]   → 4-image gallery incl. header
embeds[4..7]  (group B, url + "#more"):  image = screenshots[3..7]   → second gallery
embeds[8..9]  (group C, url + "#more2"): image = screenshots[7..10]  → third gallery (partial)
```

10 embeds total ⇒ header + up to **10 screenshots** — the card's own cap is 10 screenshots, so
"all of the media" is literally satisfied. Groups only exist when they have images; a game with 2
screenshots gets one small gallery; a game with none gets the single embed with header art.

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
  still the whisper.

## the preview envelope — `{"op":"whisper_preview"}` (Ben's parked idea, riding along)

A no-write sibling: re-render and re-send the card for **the most recent delivered whisper**
(fall back: the pick select() would make today, clearly marked), so Ben can see card changes
without spending a weekly slot.

- ZERO writes: no record, no mark, no cycle movement. Reuses the dark-gate (both faces) verbatim.
- Sends to the whisper webhook (it previews the whisper channel's rendering — an ops-channel
  preview would render under different channel settings and prove nothing), prefixed
  `🔍 *preview — not this week's whisper*` in content so it can never be mistaken for a real one.
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

## open questions (→ OMBB + Lilith, shared channel)

1. **Media coverage:** 3 gallery groups / 10 embeds = every screenshot, but a TALL message. The
   tasteful alternative: one 4-image gallery + a "＋N more on steam" note. Ben's words say ALL;
   taste says the second. Which wins? (My lean: all 10 — his sentence is explicit, and Discord
   collapses galleries compactly.)
2. **Trailer:** `video_hls_url` is an .m3u8 — not browser-playable as a bare link. Card users get
   the in-modal player; Discord can't. Proposal: `🎬 watch the trailer` markdown link → the steam
   store page (trailer autoplays at top). Anyone see a better throat?
3. **Preview recipient:** whisper webhook (marked) per above — anyone want it on the ops webhook
   instead?
