# spec: the wrapping paper 🎁 — per-gift unfurls

status: DRAFT r2 (2026-09-07) — family review integrated (OMBB routing/copy/art-structure, Lilith oracle/escaping/witness) · author: kitten · pounce arc

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
presents. **Scope of the promise (r2): "same art forever" is a GIFT-link promise.** Shelves ship ONE shared
design, and not as phasing: variants exist to distinguish multiple objects belonging to one person,
and a friend has exactly one shelf — token-hashed shelf art would distinguish it from nothing, and a
shared hash could coincidentally dress a shelf in one of that friend's gift papers, a coincidence
that looks like the system saying something (OMBB). If shelf variety ever earns its way in, hash
something meaningful (shelf size, first-gift year), never the token — **and draw shelf art from a
DISJOINT PALETTE gifts never use. Not a salt: `Friend.shelf_token` (`domain:177`) and `Link.token`
(`:184`) are distinct fields already, so the hashes are decorrelated under any keying, and
independent draws into 8 buckets still collide 1-in-8 per gift — compounding to 1−(7/8)ⁿ ≈ 33%
that the shelf matches SOME gift for the canonical three-treasure friend (Lilith's number) — a
birthday problem no salt escapes (OMBB, refuting a salt sentence r2.1 briefly carried). A disjoint palette takes the
collision to zero by construction, and it says the true thing visually: a shelf is a different
KIND of object, not another present.**

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
HTML comment markers, not tag parsing. **Marker absence gets a witness (Lilith):** if a deployed
index.html ever lacks the markers, the swap must not silently no-op forever — the lambda emits a
structured ERROR log + CloudWatch metric on anchor absence while serving the generic card, so the
degradation announces itself. The web-side vitest guards the artifact where it is BUILT; the
lambda's witness guards what actually reached the bucket — different failure, different guard. New
IAM: public-api gains `s3:GetObject` on `index.html` only. The `iam_capture` corpus is
DYNAMO-scoped by design (`crates/dynamo/tests/iam_capture.rs:1` captures x-amz-target request
shapes) — it does not see this grant. Non-dynamo grants follow the hand-written inline-policy
pattern instead (`aws-lambda.tf` ssm precedent); this S3 grant does the same: single object, no
wildcard.

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
**The unfurl's audience is the room at paste time, not the clicker** (OMBB) — every disclosure
here is graded against "safe to broadcast to a channel," not "less than the page."
Revoked/consumed/unknown tokens serve the CURRENT generic card (site-wide og block, exactly what
today's index.html serves) with 200 — never an error card in a chat. Oracle status, verified at
`public-api/src/lib.rs:624-629` (Lilith): the JSON API already returns byte-identical
`link_not_found_response()` for ANY invalid token, so the unfurl adds no valid/invalid channel —
and it is STRICTLY COARSER than the API it rides beside: the API distinguishes
`active|sealed|revoked|expired|exhausted` to a requester; the unfurl collapses every dead and
unknown state into one generic card. Caveat carried with the claim: equivalent information is not
equivalent disclosure — the API oracle requires someone to make a request; the unfurl fires because
a platform bot fetched automatically, then renders and caches the answer into a room. Same bits,
different blast radius; that asymmetry is why the card carries only label + count.

### D6 — what the meta says, exactly (copy is part of the spec)
| state | og:title | og:description |
|---|---|---|
| gift, n games | `🎁 ben wrapped something for {label} ♡` | `{n_word} treasure{s} inside, chosen for you. tap to unwrap.` |
| gift, sealed (unlock in future) | `🎁 ben wrapped something for {label} ♡` | `sealed for now. good things wait.` |
| shelf | `📚 the shelf ben keeps for {name}` | `every game he's given you, all in one warm place.` |
| dead/unknown | (today's generic block verbatim) | (generic) |

**Sealed-row rationale (r2): state yes, clock never — a date is a spoiler with a calendar attached
(OMBB). The unfurl may reveal that a sealed thing exists (the act of pasting already does); the only
temporal disclosure was the date, and you can't un-broadcast one.**

**Escaping is NEW CODE, not reuse (Lilith, measured on main):** public-api contains zero HTML
escaping; `sanitize_note` (:1102) is a control/bidi STRIPPER that leaves `" < > &` intact and has
only ever applied to `note`; `label` is a bare String passed through (:165, :663, :794). The
protection to date lived in React's JSX rendering — and the lambda has no React. Therefore:
1. A dedicated attribute-context escaper for ALL injected text — `& < > " '` — because og meta
   lands in `content="…"` and **`"` is the breakout character** element-text escaping forgets.
   **The two fields have different write-time histories and D6 names them separately (Lilith):**
   `{name}` is stripped at create by `sanitize_friend_name` (`admin-api:1032`, applied `:1056`);
   `{label}` has NO sanitizer anywhere — a length cap and nothing else. The lambda treats them
   IDENTICALLY anyway (strip format/control chars + escape + clamp, at render): render-side
   protection must not trust write-side history, and write-side history must not excuse a field.
   **Placement:** `sanitize_friend_name` already mirrors `public-api::is_spoofing_format_char`
   with a keep-in-sync comment (`admin-api:997-999`) — two implementations of one rule. The
   escaper+stripper for this arc lives ONCE, in the `domain` crate, and the render path calls it;
   this arc does NOT refactor the two existing copies (scope), but the new code adds no third.
2. **Blast radius is the live page, not the card** — D3 serves identical bytes to humans, so a
   broken attribute in `<head>` is injection for every human booting the SPA.
3. **Provenance, measured (independently by me and OMBB, agreeing):** `Link.label` has exactly one
   write path — `admin-api:757`, admin body; `public-api` holds field + two reads, no write. And
   the only validation on it is `LABEL_MAX_CHARS` (`admin-api:657`) — **a length cap bounds count,
   never structure; `" onload=x>` is eleven characters. A cap reads like validation and validates
   nothing this section cares about (OMBB).**
   `Friend.name` likewise + bidi-stripped at create (:1056); steam `personaname` flows to neither
   on main. Severity today: low — fix fully anyway. **Recorded ASSUMPTION: any feature letting a
   friend set their own display name re-triggers this section's severity review.** (The
   humble-self-login branch adds no label write — but it is 345 lines against main's 1543, so
   that is evidence about a stale snapshot, not about the design that ships.)
4. Property test (hers, adopted): a label of `" onload=x><script>` CANNOT change the tag
   structure of the output — structure-invariance, not "is escaped," so the assertion survives a
   future escaper swap.

Counts use lowercase words ("three"), capped at "a dozen" style phrasing above 12 — chat cards,
not receipts.

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

## resolutions (r2) — the r1 open-questions block is retired; every OQ re-derived from the body
- **routing**: one path, always-lambda, all viewers. Two code paths for one URL put the unfurl on
  the branch nobody can test; UA-in-cache-key fragments the cache the sniff protects; unknown-UA
  fails silently boring (OMBB). (r1's OQ1 offered an option the non-goals already forbade —
  process note: OQ blocks are re-derived from the body, never edited in place.)
- **caching**: 60s CF TTL, per-path keys. Named cost, decided not defaulted: up to 60s of
  revocation latency on the card — acceptable because the card carries only label+count and the
  page stays live-checked. INVARIANT: the token is a path segment on both routes, never a query
  param; a future query-param route must enter the cache key or it cross-serves.
- **shelf art**: one shared design, structural (see D1).
- **sealed copy**: state without clock (see D6).
- open for execution only (mine): metric shape for the marker witness (log+metric-filter vs
  put-metric), and whether the witness alarm joins aws-cloudwatch-alarms.tf in this arc or a
  follow-up.
