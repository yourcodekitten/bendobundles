# the keepsake 📸 — spec

*2026-10-05, kitten. Status: v1 draft — pounce arc, awaiting family questions. Where narrative and
**decisions** disagree, the decisions win.*

## why this exists

PRODUCT.md principle 1: *"The unwrap is the product … spend the craft budget where the friend's
heart rate is."* We spent it on the moment — the chain, the fanfare, the confetti. But the
moment **leaves nothing in the friend's hands** except a one-time humble URL they redeem and
forget. The shelf remembers for them, behind a link. Nothing travels.

Friends *"visit rarely and remember the feeling, not the UI"* (PRODUCT.md, Users). A keepsake is
the feeling made portable: a small polaroid-style card — the cover art, the title, ben's note,
**"in the attic since 2014"**, the day they unwrapped it — that they can save to their camera roll
or send to the group chat. A gift you can show someone is a gift twice.

Composition, not construction — every input is already on the friend's wire except one field.

## what it is

A **"save a keepsake ♡"** action in two places:

1. **the unwrap** — `ClaimDialog`'s `gifted` step, beneath the humble link (never above it: the key
   is the job, the keepsake is the charm — principle 5, *delight never gates*).
2. **the shelf** — each gift card on `/s/{token}`, so a gift unwrapped in July can be kept in October.

Pressing it renders a **1080×1350 PNG** (4:5, the phone-and-chat-friendly portrait) client-side on a
`<canvas>`, then:
- where `navigator.canShare({ files: [png] })` is true (phones) → `navigator.share` (camera roll,
  messages — the native sheet);
- otherwise → `<a download="keepsake-<slug>.png">` from an object URL, revoked after click.

A small preview of the card shows in-dialog before saving, so the friend sees what they keep.

## the card (layout, top → bottom)

- warm paper frame in the house palette (dark-theme-safe, not white-on-white), polaroid proportions
- **cover art**, fitted (contain, not crop — humble subproduct icons are square, steam headers 460×215)
- **title** (wrapped, ≤3 lines, ellipsised)
- **"from ben ♡"**, and ben's note in quotes when present (per-game note on curated links, else the
  link's `gift_note`; ≤5 lines, ellipsised)
- the **postmark line**: `in the attic since 2014 · unwrapped oct 5, 2026` — first clause
  presence-gated on `acquired_at`; when the gap is ≥1 year, `· waited 12 years for you`
- a tiny `bendobundles` mark, bottom corner. **No URL of any kind.**

## decisions

**D1 — the card NEVER carries a capability.** No `gift_url`, no revealed key, no link token, no
shelf token, no QR, no domain path beyond the bare wordmark. The renderer's input type **does not
have those fields** — it takes a `KeepsakeInput { title, artworkUrl, note?, acquiredAt?, unwrappedAt }`
built at the call site, so a capability cannot reach the canvas by accident. A test asserts the
type's keys exhaustively. *(A keepsake is designed to be forwarded; the gift URL is a one-time
bearer capability. That is the whole security model of this feature.)*

**D2 — art failure degrades, never blocks.** The image loads with `crossOrigin = "anonymous"`.
Measured 2026-10-05: `shared.akamai.steamstatic.com` → 200 + `access-control-allow-origin: *`;
`hb.imgix.net` → `access-control-allow-origin: *` (on a 403 probe — **re-measure on a real
`artwork_url` before building the art path**). If the image errors, times out (4s), or the canvas
would be tainted (`toBlob` throws `SecurityError`), the card renders with a **drawn placeholder**
(the house pixel-gift glyph) instead — the keepsake still saves. Principle 5.

**D3 — fonts are awaited.** Canvas text in an unloaded webfont silently falls back. Render after
`document.fonts.load()` for the faces used (Silkscreen for the wordmark/postmark, the body face for
note/title), with the same 4s cap; on timeout, render anyway in the fallback stack. ⚠️ **#261 is adjacent:** vite inlines Silkscreen's
latin-ext subset as a `data:` font and `font-src 'self'` blocks it — so a title with latin-ext glyphs
falls back for *those glyphs* on the card exactly as it already does on the page. Not this feature's
to fix; the card must not be worse than the page, and it isn't.

**D4 — shelf gets `acquired_at`.** `ShelfGiftView` (public-api `assemble_shelf`) gains
`acquired_at: Option<rfc3339>`, `skip_serializing_if = None`, read from the `Game` record the
function **already holds** (`games.get(&c.game_id)`). No new reads, no new writes. The unwrap path
already has `GameView.acquired_at`.

**D5 — the unwrap date.** Shelf: `unwrapped_at` (claim.created_at). Unwrap dialog: the client's
"now" at success — the claim was created seconds ago; exact server time is not worth a wire change.
Rendered in **America/New_York-agnostic local date** (the friend's own zone; a date, not a time).

**D6 — reduced motion / no-JS-API environments.** No animation in the feature at all beyond the
existing button affordance. If `HTMLCanvasElement.prototype.toBlob` is missing (ancient browsers),
the button does not render — never a broken button.

**D7 — no backend rendering, no storage.** Nothing is uploaded, stored, or logged. The PNG exists
only in the friend's browser until they save it. No new lambda, no S3, no CSP change (`img-src`
already allows both art hosts; `blob:` downloads are not governed by `img-src`).
📏 **Measured 2026-10-05, live header == `terraform/aws-cloudfront.tf` `site_csp`:** `img-src 'self'
https://*.steamstatic.com https://hb.imgix.net` — **no `blob:`, no `data:`**. ⇒ the preview **is the
`<canvas>` element itself**, scaled by CSS; never an `<img src=blob:…>` (it would be blocked). Still no
CSP change. The download/share path hands a `Blob`/`File` to the browser, which `img-src` does not govern.

## out of scope

- admin-side keepsakes (the scrapbook is ben's; this is the friend's)
- animated/video keepsakes, stickers, multiple layouts, friend-chosen themes
- sharing to a URL (would need hosting + a capability — explicitly D7/D1's opposite)

## open questions (for the family)

- **Q1 (taste):** keepsake on the unwrap dialog as well as the shelf, or **shelf only** — is a second
  button on the gifted step clutter at the one moment that must be about the key?
- **Q2 (privacy):** ben's note is written to ONE friend. A keepsake makes it forwardable. Include it by
  default, include behind a toggle on the preview ("include ben's note"), or never?
- **Q3 (infra, OMBB):** anything in the CloudFront/CSP layer that would make `blob:` object-URL downloads
  or `navigator.share` with files misbehave that I would not see from the config alone?

## success

A friend on a phone unwraps, taps "save a keepsake ♡", and the native share sheet offers a card that
looks like a polaroid of the gift — art, title, ben's words, *waited 12 years for you* — and nothing
on it that could be used to claim anything.
