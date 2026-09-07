# spec: the wrapping paper 🎁 — per-gift unfurls

status: DRAFT r1 (2026-09-07) · author: kitten · pounce arc

## why

PRODUCT.md's first sentence about friends: *"They arrive from a chat message ben sent them."*
That chat message is the true first screen of the gift — and today every link ben pastes unfurls
as the same generic card (`web/index.html:21-33`, one static `og.png` for the whole site). The
product's grammar is *chosen-for-you*; the first moment a friend sees is *same-for-everyone*.

Chat unfurl bots do not execute JavaScript, and CloudFront's `spa_rewrite` function sends every
extensionless path — `/l/<token>`, `/s/<token>` — to the static `index.html`. **No client-side
code can ever fix this.** Personalizing the unfurl requires serving per-token HTML from the
origin. That is this feature.

## what the friend sees (the whole feature in two unfurls)

Pasting a **gift link** `/l/<token>` into Discord/iMessage/Slack unfurls as:

> **🎁 ben wrapped something for {label} ♡**
> three treasures inside, chosen for you. tap to unwrap.
> `[wrap-art image: a pixel-art wrapped present in one of 8 muted-earth papers]`

Pasting a **shelf link** `/s/<token>` unfurls as:

> **📚 the shelf ben keeps for {name}**
> every game he's given you, all in one warm place.
> `[wrap-art image: shelf variant]`

The unfurl is the **wrapped box, never the contents**: no game titles, no cover art, no store
energy. The unwrap ceremony (`ClaimDialog`'s chain) stays intact — the card in chat is the
wrapping paper seen across the room, and opening the link is still the reveal. At most a count
("three treasures") — an anticipation cue, not a spoiler.

## design decisions

### D1 — wrap color is deterministic by token, via the existing title-hash mechanism
DESIGN.md's Title-Hash Rule: deterministic muted-earth palette (heather, slate, moss, mustard,
rust, mauve, pine, clay). Wrap art ships as **8 pre-designed pixel-art present PNGs** (1200×630),
one per palette token, picked by the same style of stable hash over the link token. Consequence
worth the sentence: **the same gift always wears the same wrapping paper** — re-pasting the link
re-unfurls identically, and two different gifts to the same friend look like two different
presents. Shelf cards get the same treatment with a shelf-flavored composition (8 more PNGs, or
1 shared shelf design in phase 1 — see OQ3).

### D2 — meta text carries the personalization; images are pre-baked
Per-name *rendered* images (label drawn into the PNG) are explicitly **out of scope** for this
arc: they need a rasterizer in the lambda (font rendering, bidi/emoji in names — the gift-shelf
arc's sanitize lessons) for marginal warmth over a personalized *title* + varied art. The title
line is what chat clients render biggest anyway. Recorded as the natural v2.

### D3 — the served HTML is the deployed `index.html` with the og block swapped
The unfurl route must return HTML that ALSO boots the SPA for human clicks (bots and humans get
identical bytes — no UA cloaking anywhere). Vite hashes bundle filenames, so the lambda cannot
embed `index.html` at build time without deploy-order coupling. Instead: **public-api fetches
`index.html` from the web S3 bucket** (in-memory cache, short TTL ~60s), replaces the delimited
og block (`<!-- open graph … -->` … end marker added by this arc) with per-token meta, and
serves it. Web deploys keep working with zero coordination; the swap is anchored on explicit
HTML comment markers, not tag parsing. New IAM: public-api gains `s3:GetObject` on
`index.html` only — the iam_capture corpus gains the grant (gift-shelf pass-2 lesson: corpus
must see every new store/client call).

### D4 — routing: `/l/*` and `/s/*` become CloudFront API-origin behaviors
Two new ordered cache behaviors (`/l/*`, `/s/*` → api origin), placed after the `/api/*`
behaviors. The `spa_rewrite` function never sees them (it is behavior-scoped, per its own
comment). Caching: short TTL (60s) keyed on path — per-token cache entries are fine; the URLs
are the capability. **My lean on the latency/availability trade** (open question OQ1): serve
ALL viewers from the lambda, no UA sniffing — a UA roster rots (this repo's own
hand-written-roster scar tissue), rust lambda cold starts are small, and the SPA is already
useless when public-api is down, so the added SPOF surface is thin. Failure posture: if the
lambda 5xxes, CloudFront's custom error responses serve the themed fallback.

### D5 — dead tokens unfurl warm and generic
Revoked/consumed/unknown tokens serve the CURRENT generic card (site-wide og block, exactly
what today's index.html serves) with 200 — never an error card in a chat, and no new oracle:
`/api/l/<token>` already distinguishes valid tokens to any holder; the unfurl adds no channel
that the JSON API doesn't already have. Content-timing note: personalized-vs-generic is
observable to a token-guesser exactly as far as the existing API is — acceptable, same trust
model (the token IS the auth).

### D6 — what the meta says, exactly (copy is part of the spec)
| state | og:title | og:description |
|---|---|---|
| gift, n games | `🎁 ben wrapped something for {label} ♡` | `{n_word} treasure{s} inside, chosen for you. tap to unwrap.` |
| gift, sealed (unlock in future) | `🎁 ben wrapped something for {label} ♡` | `sealed until {date-ish}. good things wait.` |
| shelf | `📚 the shelf ben keeps for {name}` | `every game he's given you, all in one warm place.` |
| dead/unknown | (today's generic block verbatim) | (generic) |

`{label}`/`{name}` are HTML-escaped and length-clamped (the friend-name sanitize rules from the
gift-shelf arc apply verbatim — bidi/format chars stripped). Counts use lowercase words
("three"), capped at "a dozen" style phrasing above 12 — chat cards, not receipts.

## non-goals
- per-name rendered og images (v2, see D2)
- unfurls for `/admin/*` or any other route
- UA-conditional responses (identical bytes to all fetchers of a path)
- changing what `/api/*` returns; this is a read-only presentation layer

## testing
- unit (public-api): meta injection anchored on markers survives an index.html byte-change test;
  escape/clamp property tests on label; hash→variant stability pinned (same token, same wrap,
  forever — a golden test); dead-token path returns generic block byte-identical to template.
- integration: S3 fetch path against moto (bucket fixture with a real built index.html);
  cache TTL behavior (stale template swapped within TTL window).
- web: zero code changes expected; one vitest asserting index.html carries the og block
  MARKERS (the lambda's anchor is a contract on web's artifact — test it where it's built).
- CI: existing suite lanes; no new workflow.

## deploy shape (runbook #217 applies)
terraform: 2 CF behaviors + 1 IAM statement + (no new lambda). Derive the tfvars variable set
(`grep -hoP 'variable "\K[a-z0-9_]+' terraform/*.tf`); this arc is NOT code-only, so a non-zero
destroy is READ-EVERY-LINE, not auto-STOP (CF behavior edits can legitimately replace). Zips
from green MAIN run. Wrap PNGs ship in `web/public/` (S3, long-cache) — art is a web asset,
not a lambda payload.

## open questions (family, step 2)
- **OQ1**: all-viewers-through-lambda (my lean, D4) vs edge UA-sniff bots-only vs something
  smarter? The trade is first-hop latency/availability vs a UA roster that rots.
- **OQ2**: is 60s CF caching of personalized HTML acceptable, or should unfurl HTML be
  no-cache (every paste hits the lambda)?
- **OQ3**: shelf art — 8 variants like gifts, or one shared shelf design in phase 1?
- **OQ4**: does the sealed-gift state deserve its own copy row (D6 line 2), or is that a
  content leak of "there is a timed thing"? (my read: the SealedGift page already shows the
  countdown to the token holder; the unfurl says less than the page.)
