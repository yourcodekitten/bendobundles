# the gift shelf 🎁📚 — spec

*2026-09-04, kitten. Status: PROPOSAL — family review pending. Where narrative and **decisions**
disagree after review, the decisions win.*

## why this exists

PRODUCT.md's own anti-reference line names it: *"It's a gift shelf, never a shop."* We have the
shop-that-isn't; we never built the shelf.

Today a gift's whole life is one link: a friend arrives, unwraps, says thank you — and the moment
evaporates. `Link.label` strings are the only place friends exist at all ("sarah bday", "sarah
xmas" — the same person, scattered). PRODUCT.md says friends *"visit rarely and remember the
feeling"* — but the feeling has nowhere to live between visits, and fifteen years of generosity
has no memory of itself.

The gift shelf is a **persistent, warm, per-friend page**: every game ben ever gave them, cover
art and year and his note and their thank-you back — a small attic shelf that is *theirs*. A
friend keeps one URL and their collection grows quietly under it. The unwrap stays the product;
the shelf is where the unwraps go to live.

## what it is

- **A friend page at `/s/{shelf_token}`** — no account, same trust model as invite links: the URL
  is the key. Greeting in the brand voice ("ben's shelf for ⟨name⟩ ♡"), then the gifts: cover art,
  title, the year they unwrapped it, ben's `gift_note` from that link, the friend's `thank_note`
  back.
  Oldest first — a shelf accretes; the newest gift is the one you scroll to with everything you've
  been given behind it. (Accretion is the normal motion, not an enforced invariant — reassignment
  is mutable admin state and can move a gift between shelves; see admin-api.)
- **A lightweight `Friend`** in the data model: `{id, name, shelf_token, created_at}`. Links gain
  an optional `friend_id`. That's the whole identity system — no auth, no email, no profile.
- **Admin plumbing sized to the workbench**: create a friend (mints the shelf token), pick a
  friend when cutting a link, assign/reassign a friend on existing links (this *is* the backfill —
  no special migration screen), copy the shelf URL, revoke+reissue a token.

### non-goals (decided, not omitted)

- **No bell on shelf visits.** A friend revisiting their own shelf must never page ben — same
  consent logic as the browse-doesn't-ring decision in spec-attic-bell.md.
- **No unclaimed/chosen-but-unopened games on the shelf.** Only claimed gifts appear. A "you
  haven't opened this yet" row is pressure, and pressure is shop grammar. (Family review agreed —
  and located the warm version: "chosen and waiting" is BEN's feeling, which belongs to the
  deferred ben-facing scrapbook, not the friend's page.)
- **No wishes/requests, no whisper integration, no ben-facing scrapbook view** — each is a future
  spec if it earns one. v1 is the friend's shelf, read-only.
- **No friend deletion in v1.** Revoke the shelf token and the page is gone; deleting the
  record would dangle `friend_id`s on links for no user-visible gain. If real deletion is ever
  wanted it arrives with its own cleanup design.
- **No pagination.** One friend's claimed gifts across 15 years is dozens, not thousands; a single
  query page bounded far under the 1MB item cap. Stated so the future knows it was considered.

## architecture

**Domain** (`crates/domain`):
- `Friend { id: String, name: String, shelf_token: String, created_at }` — new record type.
- `Link.friend_id: Option<String>` — following the `gift_note` pattern exactly: authoritative in a
  top-level dynamo attribute written only by a scoped update (`set_link_friend`), stripped from
  the stored `body` blob, overridden on read, `#[serde(default)]` so every pre-existing record
  reads `None`. **This pattern is MECHANICALLY REQUIRED, and the reason is the WRITE PATH, not
  the index** (first stated wrong here — "a GSI key must be top-level" is true but the index key
  could simply *derive* from a body field, as `gsi1pk` already does; OMBB's correction, family
  review): **the claim transaction writes `SET body = :b` from a pre-transaction read
  (`lib.rs:1208`), so a body-only `friend_id` is silently REVERTED by the friend opening their
  gift — while a separate `gsi3pk` survives, desynchronising index from record.** `lib.rs:410`
  already prescribes the gift_note recipe for every future editable field for exactly this.
  ⇒ **testable, and tested: `friend_id` must survive a claim** (see testing). Editability and
  no-copy-at-rest ride along.

**Dynamo** (`crates/dynamo`, single table):
- `FRIEND#<id> / META` — the friend record.
- `SHELF#<token> / META` — pointer item `{friend_id}`; shelf resolution is a direct pk get, same
  shape as link tokens. **Revoke = one transaction: delete pointer + REMOVE `friend.shelf_token`**
  (the friend record must not keep a dead capability at rest — the same no-copy rule the
  `gift_note` pattern exists for). **Reissue = one transaction: delete OLD pointer + put new
  pointer + set `friend.shelf_token`** — the old URL dies atomically with the new one's birth,
  which is exactly what the test list asserts ("reissue invalidates old token").
- **Sparse GSI** (`gsi3pk = FRIEND#<friend_id>`) on LINK items, present only when `friend_id` is
  set → `list_links_for_friend`. Sparse means unassigned links simply don't exist to the shelf —
  backfill is optional per-link, zero migration. **Projection: `ALL`** (the handler needs whole
  links; matches gsi1/gsi2 precedent) — noting that ANY projection carries the table keys, so a
  gsi3-shaped exposure yields link tokens at every projection level; the projection choice
  decides whether the NOTES ride beside them, never whether the tokens do (Lilith). **And name
  the true new shape: gsi3 is a second home for link tokens, organised BY FRIEND** — the base
  table scatters them, this index answers "every token belonging to this person" in one query.
  That organisation IS the feature; it is also a fact about exposure, recorded here rather than
  discovered in an incident.
- Token minting: same idiom links use — two uuid-v4 concatenated, 64 hex, ≥128 bits (currently
  inlined twice in admin-api at :240/:707; a third site extracts it into a helper all three call).

**public-api**:
- `GET /api/s/{token}` → `404` unknown/revoked · `200 {name, gifts: [...]}` where gifts =
  friend's links (gsi3) → `claims_for_link` each → **`ClaimState::Fulfilled` only** (`Pending`
  isn't a gift yet; `Failed` never was; `Compensated` is a gift that was *taken back* by
  reconcile — rendering it would be a lie) → game + art. **`handle_game_detail` is link-scoped
  (resolves a link from its path token first — verified lib.rs:1316) and cannot be reused as a
  route; the detail-assembly under it (game record + steam cache join) gets extracted into a
  helper both handlers call.** Each gift: `{title, artwork_url, unwrapped_at
  (claim.created_at — named for what it IS: the friend's unwrap moment, which is the memory this
  page serves; "when ben chose it" is link.created_at and belongs to the future ben-facing
  scrapbook), gift_note, thank_note, detail…}`. Read-only; thank-yous continue to live on link pages.
- **Partial failure is whole failure**: if any sub-fetch (links, claims, games, cache) errors,
  the response is a 500 in the brand's soft voice — a silently incomplete shelf reads as "ben
  gave me less than he did," which is worse than an error.

**admin-api**:
- `POST /api/admin/friends` (create, mints token) · `GET /api/admin/friends` (list) ·
  `PATCH /api/admin/friends/{id}` (rename · reissue token) — input validation per the
  create-link 422 conventions.
- `set_link_friend` wired to the existing link-edit surface (assign/reassign/clear).
  **Reassigning a link moves its whole gift history between shelves — deliberately** (it exists
  to fix mis-assignment); the old shelf visibly shrinks. Admin copy says so at reassign time.

**web**:
- Friend surface: `/s/{token}` → `ShelfPage` — new composition, inheriting the attic aesthetic
  (soft edges, cover art, lowercase warmth; `SealedGift`/`ThanksCard` are the register to match,
  not necessarily components to import). Empty state in-voice: a shelf waiting for its first
  story. Errors soft, like LinkPage's.
- Admin: friend picker on link create, friend column/selector on the links list, a small friends
  panel (create/rename/copy-URL/reissue).

## content & security

- **Bearer capability URL — token stored raw in the pk, and the warrant is scoped to what the
  token UNLOCKS.** Two earlier warrants died in review: "links and shelves are symmetric" (no —
  a link token is *spent*, a shelf token is *permanent and appreciates*: Lilith) and "game keys
  in CLAIM items dominate the exposure" (no — keys are *decaying* assets; the dominance holds at
  t=0 and inverts as they're redeemed, and key-only channels — streams, query logs, exports,
  projections — leak pks *without* bodies at all: Lilith again). What stands: **the asset behind
  a shelf token is a keepsake page** — a first name, game titles, warm notes. Privacy-sensitive
  and worth protecting; not redeemable, not money. v1 accepts raw-in-pk for that asset class,
  consistent with every other token this table keys.
  **Reissue is the remedy and it is OPERATOR-INITIATED WITH NO DETECTION PATH** (OMBB): nothing
  can announce a leaked shelf URL — no bell on visits (by design), no self-announcing failure
  like a dead key. Do not read "reissue exists" as coverage; it fires when a human notices.
  If the table's secret-at-rest posture is ever hardened (#88's direction), shelf tokens join
  that migration first — they are the longest-lived capability the table holds in the clear.
- **No cross-friend leakage by construction**: resolution starts at one pointer item and fans out
  only through `FRIEND#<id>`-keyed queries.
- **Unknown and revoked are the same 404** — a probe learns nothing about which tokens ever lived.
- The friend's `name` renders on an unauthenticated page: admin copy should say so at create
  ("first names are plenty").

## deploy notes

- **`/s/{token}` needs zero CloudFront change — verified**: `spa_rewrite` rewrites every
  extensionless URI to `/index.html` (aws-cloudfront.tf:13), so the new route is client-side
  only. Steam art on the shelf is already within the CSP (`img-src *.steamstatic.com`, #81).
  The steam-login `ctx` allowlist stays untouched: no steam flow exists on shelves.

- New GSI on the existing table = in-place terraform update. **Run the runbook** (#217): derive
  the tfvars variable set, and any non-zero `destroy` on this code-only deploy is STOP.
- Zero data migration: serde defaults + sparse GSI mean every existing record is already valid.

## testing

- domain: serde round-trips for `Friend`, `Link.friend_id` default-on-missing.
- dynamo: store tests per house pattern (real-SDK path in CI) — pointer resolution, sparse-GSI
  list, revoke/reissue, `set_link_friend` scoped update + body-strip + read-override, and
  **`friend_id` survives a claim** (assign → claim through the real transaction → assert the
  assignment and the gsi3 row both intact; this is the write-path-revert reason made falsifiable).
- public-api: handler tests — 404 unknown, 404 revoked, 200 shape, claimed-only filtering,
  cross-friend isolation (two friends, two links, no bleed).
- admin-api: validation 422s, friends CRUD, reissue invalidates old token.
- web: component tests per house pattern (ShelfPage states: loading/empty/populated/404).

## open questions — RESOLVED in family review (2026-09-04, Lilith)

1. **claimed-only** — stands; the warm version of "waiting for you" is ben's, on the deferred
   scrapbook surface.
2. **`friend_id` pattern** — gift_note recipe required, but the first stated reason was a
   non-sequitur (an index key can DERIVE from a body field — gsi1pk does). The real mechanism is
   the claim path's `SET body` revert (OMBB, from lib.rs:1208 + my own :410 note); stated at the
   definition, enforced by a test.
3. **raw vs hashed shelf token** — raw, re-argued on the table's actual contents (claims hold
   `gift_url`/`revealed_key`) rather than my refuted symmetry premise; lifetime asymmetry
   acknowledged, reissue is the leak response.
