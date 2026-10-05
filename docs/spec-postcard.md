# the postcard 🖼️ — spec

*2026-10-05, kitten. Status: **v7 — AMENDED AFTER OMBB's step-5 SIGN-OFF**, to match what ships after execution + review passes 1 and 2 (headline, desktop share, InvalidStateError + re-tap guard, failure state, D9 save weight, real italic). Every amendment below is marked *(v7)*. v6 — OMBB step-5 verdict (APPROVE WITH CHANGES) + Lilith's {blob, artUsed} integrated; D13. v5 — plan-review amendments (line caps, fonts, postmark example, shelf invariant). v4 — OMBB's Q3 (infra clear; user activation, D10) + Lilith's staleness catch (D11) integrated. v3 — Lilith's Q1/Q2 answers + D1 limit + heading integrated; renamed from "postcard" (collides with the scrapbook's *postcard*, docs/spec-scrapbook.md:37); Q3 (OMBB) open. Where narrative and
**decisions** disagree, the decisions win.*

## why this exists

PRODUCT.md principle 1: *"The unwrap is the product … spend the craft budget where the friend's
heart rate is."* We spent it on the moment — the chain, the fanfare, the confetti. But the
moment **leaves nothing in the friend's hands** except a one-time humble URL they redeem and
forget. The shelf remembers for them, behind a link. Nothing travels.

Friends *"visit rarely and remember the feeling, not the UI"* (PRODUCT.md, Users). A postcard is
the feeling made portable: a small polaroid-style card — the cover art, the title, ben's note,
**"in the attic since 2014"**, the day they unwrapped it — that they can save to their camera roll
or send to the group chat. A gift you can show someone is a gift twice.

Composition, not construction — every input is already on the friend's wire except one field.

## what it is

A **"send a postcard ♡"** action in two places:

1. **the unwrap** — `ClaimDialog`'s `gifted` step, beneath the humble link (never above it: the key
   is the job, the postcard is the charm — principle 5, *delight never gates*).
2. **the shelf** — each gift card on `/s/{token}`, so a gift unwrapped in July can be kept in October.

Pressing it renders a **1080×1350 PNG** (4:5, the phone-and-chat-friendly portrait) client-side on a
`<canvas>`, then:
- where `navigator.canShare({ files: [png] })` is true → `navigator.share` (the native sheet). *(v7: that is
  phones AND desktop Chrome/Edge and macOS Safari — not phones only, as v1–v6 said);*
- otherwise → `<a download="postcard-<slug>.png">` from an object URL, revoked after click.

A small preview of the card shows in-dialog before saving, so the friend sees what they keep.

## the card (layout, top → bottom)

- warm paper frame in the house palette (dark-theme-safe, not white-on-white), polaroid proportions
- **cover art**, fitted (contain, not crop — humble subproduct icons are square, steam headers 460×215)
- **title** (wrapped, ≤2 lines at 64px, ellipsised)
- **"from ben ♡"**, and — only when the friend ticks D8's toggle — ben's note in quotes (per-game note on curated links, else the
  link's `gift_note`; ≤3 lines at 36px italic, ellipsised)
- *(v7)* the **headline**: `waited 12 years for you` — its own single line, 44px Pixelify in give-pink,
  above the postmark; absent under one whole year or when `acquired_at` is unknown. *(v1–v6 put it at the
  end of the postmark; the first real-browser render buried the card's best line in small print and
  orphaned "for you", so it was promoted.)*
- the **postmark line**: `in the attic since mar 2014 · unwrapped oct 5, 2026` (month+year via the shared `postmark()` helper, so it reads exactly like the site's own postmark) — first clause
  presence-gated on `acquired_at`
- a tiny `bendobundles` mark, bottom corner. **No URL of any kind.**

## decisions

**D1 — the card carries no capability FIELD** (and its free-text note is unvetted — see the limit below). No `gift_url`, no revealed key, no link token, no
shelf token, no QR, no domain path beyond the bare wordmark. The renderer's input type **does not
have those fields** — it takes a `PostcardInput { title, artworkUrl, note?, acquiredAt?, unwrappedAt }`
built at the call site, so a capability cannot reach the canvas by accident. A test asserts the
type's keys exhaustively. *(A postcard is designed to be forwarded; the gift URL is a one-time
bearer capability. That is the whole security model of this feature.)*
⚠️ **WHAT D1 GUARANTEES, AND WHAT IT DOES NOT (Lilith, 2026-10-05):** it rules out capability
**FIELDS**. It cannot rule out capability **CONTENT**: `note` is free text ben types, and the day he
writes *"here's the DLC code: XXXX"* or pastes a link, the exhaustive-keys test stays green and the
key leaves in a PNG. ⇒ **the note is UNVETTED content**, and D8 (default OFF) is the mitigation, not
a proof. The claim is *"no capability field"*, never *"no capability"*.

**D2 — art failure degrades, never blocks.** The image loads with `crossOrigin = "anonymous"`.
Measured 2026-10-05: `shared.akamai.steamstatic.com` → 200 + `access-control-allow-origin: *`;
`hb.imgix.net` → `access-control-allow-origin: *` — first on a 403 probe, then **re-measured on a
REAL prod `artwork_url`: 200 `image/png` + `ACAO: *`** (2026-10-05T07:0x-04:00), **and again WITHOUT an
`Origin` header: 200 + `ACAO: *`** (OMBB's cached-copy-taint question — the page's plain `<img>` may
seed the cache our `crossOrigin` load reuses; an unconditional `*` makes that safe). Steamstatic: same,
measured by OMBB. ⚠️ **Art loads ONLY via `<img crossOrigin="anonymous">`, never `fetch()`** —
`connect-src` does not list `hb.imgix.net`. If the image errors, times out (4s), or the canvas
would be tainted (`toBlob` throws `SecurityError`), the card renders with a **drawn placeholder**
(the house pixel-gift glyph) instead — the postcard still saves. Principle 5.

**D3 — fonts are awaited.** *(v7: the note's italic is now a REAL face — `@fontsource-variable/chivo/wght-italic.css` is imported in `main.tsx`; before, both the page and the card synthesised it.)* Canvas text in an unloaded webfont silently falls back. Render after
`document.fonts.load()` for the faces used (Silkscreen for the **wordmark only** — a pixel face is illegible across a long postmark line, so the
postmark uses Chivo; Pixelify for the title; Chivo for from-line and note), with the same 4s cap; on timeout, render anyway in the fallback stack. ⚠️ **#261 is adjacent:** vite inlines Silkscreen's
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

**D8 — ben's note is OFF by default, behind a toggle (Lilith, Q2).** He wrote it to ONE person; a
postcard is built to be forwarded, so the default is the private state and putting the note on the
card is **the friend's** choice. The preview has an *"include ben's note"* checkbox (absent entirely
when there is no note), and **the preview redraws live from the same render call the save uses** — so
what they see is exactly what leaves their hands. One render function, one input; never a preview
path and a save path that could disagree.

**D9 — placement on the unwrap (Lilith, Q1): both surfaces.** *(v7, review 2: D9 governs the ENTRY. Once
the friend opens the panel, its save is a real button — but an **outline** one, never a fill; the filled
buttons on that screen stay the key's.)* *(v7: "never during `celebrating`" holds by construction — the
entry renders only in the `gifted` step — and is not separately tested, since the test env forces reduced
motion and skips `celebrating`.)* On the gifted step it sits **below the
key block, only once the key is revealed** (never during `celebrating`), with a **link's visual weight,
not a button's** — the key's two buttons stay the only buttons. Saving at the peak of the moment is
the point; the shelf is the October backup.

**D10 — the tap does nothing but `share` (OMBB, Q3).** `navigator.share` needs **transient user
activation**; a tap handler that first awaits fonts + art (D2/D3's 4s caps) spends it, and Safari throws
`NotAllowedError`. ⇒ **fonts and art load ONCE, when the preview opens.** Every later render — including
D8's toggle — is a **synchronous canvas draw + `toBlob`**, cheap. The tap handler's only await-free work
is `navigator.share({ files: [file] })` / the anchor click.
- **`AbortError` = the friend cancelled the sheet. It is NOT a failure: no download fallback, no error
  copy.** *(v7)* **`InvalidStateError` = a sheet is already open — also no download** (review 1: a double tap
  shared AND downloaded). A synchronous re-tap guard ignores a second tap while a share is pending. Any
  OTHER share rejection → fall back to the download path, quietly.
- No `Permissions-Policy` header is set today, so `web-share` defaults to `self`. **If one is ever
  added it must carry `web-share=(self)`** — recorded in the CSP comment block in `aws-cloudfront.tf`.

**D11 — a blob is valid only for the input it was rendered from (Lilith, catching D10 against D8).**
Pre-rendering makes the blob a CACHE, and a cache can be stale: toggle the note off, tap fast, and *"one
input, an older render"* ships the note the friend just removed. ⇒ the rendered artifact is stored
**as a pair `{ key, blob }`**, where `key` is a deterministic serialization of the full `PostcardInput`
(**including** the toggle's effect, i.e. whether `note` is present). Save is **enabled only when
`artifact.key === keyOf(currentInput)`**; every input change re-renders, and a completion whose key no
longer matches the current input is **discarded**, never stored. The tap sends `artifact.blob` only
after re-checking the key in the same synchronous handler.

**D12 — the shelf gains exactly one affordance, and it is not a claim.** `ShelfPage.test.tsx` pinned
the shelf as having no buttons at all (*"a read-only keepsake page"*). The invariant that matters is
**no claim/action on the shelf**; the postcard entry is the one deliberate exception (D9, family-ruled),
and the test is narrowed to say exactly that rather than deleted.

**D13 — each postcard carries the note its OWN surface shows (OMBB minor 5, accepted, not fixed).**
The unwrap card uses the curated per-game note when there is one (else the link's `gift_note`); the
shelf card uses the shelf's `gift_note`, because that is the only note the shelf wire and page carry.
So on a curated link the two postcards of the same gift can differ — **each matches the page the friend
is looking at**, which is the property D8 protects ("they see exactly what leaves their hands"). Putting
curated notes on the shelf is a shelf feature, not a postcard one; out of scope here.

**D2 addendum (Lilith):** when a taint forces the art-less retry, the preview is **repainted without
the art before save enables** — the art obeys "what you see is what you keep" exactly like the note.

**D14 *(v7)* — failure is soft, and never takes the key with it (review 1).** A preview paint that throws,
or a render that rejects, shows *"couldn't make the postcard this time"* in a persistent `role="status"`
region and leaves save disabled; it never escapes the panel (there is no error boundary above it, and in the
unwrap dialog an escaped throw would unmount the one-time gift url). Encode canvases are released to 0×0
after every attempt (iOS caps total canvas memory). Wrapping cuts by code point, never through an emoji.
The preview is `role="img"` named `postcard of <title>, from ben`; the panel's save is `save my postcard ♡`
(distinct from the entry's `send a postcard ♡`).

## out of scope

- admin-side postcards (the scrapbook is ben's; this is the friend's)
- animated/video postcards, stickers, multiple layouts, friend-chosen themes
- sharing to a URL (would need hosting + a capability — explicitly D7/D1's opposite)

## open questions (for the family)

- ~~Q1~~ → **D9** (Lilith). ~~Q2~~ → **D8** + D1's limit (Lilith).
- ~~Q3~~ → **infra blocks nothing** (OMBB) + D10. Original question: **(infra, OMBB):** anything in the CloudFront/CSP layer that would make `blob:` object-URL downloads
  or `navigator.share` with files misbehave that I would not see from the config alone?

## success

A friend on a phone unwraps, taps "send a postcard ♡", and the native share sheet offers a card that
looks like a polaroid of the gift — art, title, ben's words, *waited 12 years for you* — and nothing
on it that could be used to claim anything.

## naming note

Called *the keepsake* through v2. **Renamed** because `docs/spec-scrapbook.md:37` already defines a
**keepsake card** (ben's admin-side opened-gift card) and `web/src/postmark.ts`'s `waitedYears` doc names
it as a caller. One word for two features in one repo makes every grep ambiguous. **Inherited from that
neighbour, deliberately:** `waitedYears(acquired_at, unwrappedAt)` MUST inject the unwrap instant as
`now` — a default-now call drifts +1 every january (postmark.ts's own comment).
