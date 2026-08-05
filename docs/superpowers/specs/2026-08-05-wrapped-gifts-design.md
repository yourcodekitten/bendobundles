# wrapped gifts — scheduled-unlock invite links

**date:** 2026-08-05 · **author:** code kitten · **status:** implemented (PR pending)

## the idea

`expires_at` has no twin. a link can *die* on a schedule but it can't be *born* on one.

wrapped gifts fixes that: ben preps a gift link early (a birthday, a holiday, a "you passed your
defense" present), sends it whenever he wants, and until the unlock moment the friend sees a
**wrapped present with a countdown** — label visible, contents sealed. at the moment, it unwraps
into the normal gift shelf and the whole existing claim flow takes over.

this is the product's own thesis pointed at time: *the unwrap is the product.* today the unwrap
is spatial (open link → see games). wrapped gifts makes it temporal too — anticipation is part
of the ceremony. "do not open until" is the oldest gift magic there is.

## user stories

- **ben** cuts a link for maya's birthday on tuesday, sets it to unlock saturday 00:00 ET, drops
  it in their DM tuesday night, and goes to bed. nothing to remember on saturday.
- **maya** opens the link wednesday: a wrapped pixel present with her name on the tag and a
  countdown. she comes back saturday (or keeps the tab open past midnight): it unwraps, and the
  normal chosen-for-you shelf — note, games, claim — is there.
- **a devtools-curious friend** (they're gamers; they WILL peek) inspects the network tab while
  sealed and learns *nothing* about the games or the note. the tease survives curiosity.

## design decisions (each enforced, none cosmetic)

### 1. domain: `unlock_at` on `Link`, `Sealed` on `ClaimRefusal`

- `Link.unlock_at: Option<OffsetDateTime>` — serde is the **`thanked_at` combo** (`default`
  + `rfc3339::option` + `skip_serializing_if = "Option::is_none"`; gate B1): absent on every
  pre-existing record ⇒ `None` ⇒ unchanged behavior for all current links, and None never
  serializes even a null key — which is what lets the stripped body blob be key-free and the
  body-strip test assert absence rather than null-ness.
- `Link::can_claim(now)` gains the check: `unlock_at > now ⇒ Err(ClaimRefusal::Sealed)`.
  ordering: revoked → **sealed** → expired → exhausted (a revoked gift stays revoked; a sealed
  link that also expired is a config error surfaced as sealed until unlock, then expired —
  see validation, which prevents creating that state).
- `ClaimRefusal::Sealed` is a new enum variant — the compile-time-exhaustive matches in
  public-api (claim refusal → 409 text, and the state-word derivation) are *forced* to decide,
  which is exactly the safety net the codebase already built for this moment.

### 2. wire: `state: "sealed"` withholds the payload server-side

`LinkView.state` is the single liveness word on the wire; it gains `"sealed"`. while sealed:

- `games: []` and `claims: []` — **empty, not filtered.** the sealed payload never carries
  titles, art, counts beyond claims_allowed, or anything derivable about the contents.
- `gift_note` **withheld.** the note is part of the gift; it unwraps with the shelf.
- new field `unlocks_in_seconds: u64` (present only when sealed) — server-computed remaining
  time, **ceiled** (never arrives early; sealed ⇒ ≥ 1). the client counts down from
  *remaining*, never from comparing wall clocks, so a friend's skewed device clock can't fake
  an early unwrap or show a negative countdown. at zero the client **refetches — it never
  self-unseals** (it holds no payload to unseal into, structurally); if the server still says
  sealed, back off quietly, no error flash.
- new field `unlocks_at` (rfc3339, present only when sealed) — for rendering "opens saturday"
  prose next to the ticking numbers.
- sealed responses carry `Cache-Control: no-store` — a cached sealed 200 outliving the unlock
  moment would pin a countdown past midnight (lilith). `claims_allowed`/`claims_used` stay in
  the sealed payload as the tease ("3 gifts waiting"), decided deliberately: ben sets the
  count by hand; it derives nothing about the catalog.

claim attempts while sealed get the same 409 shape as the other refusals, with soft brand
voice ("this gift is still wrapped ♡" territory — final copy at implementation).

**the fifth gate (found during family review — OMBB):** `handle_game_detail` resolves the
link but never consults `can_claim` at all — today a REVOKED link still serves game detail
for any listable game. that's a live pre-existing hole (filed as its own issue), and it is
exactly the surface the exhaustive-match socket can't protect. this feature adds the gate
explicitly: detail serves iff the games list is visible (active + exhausted), refuses
revoked/expired/**sealed** with the same byte-identical 404 the endpoint already uses for
inaccessible games (no oracle). the fix lands as the FIRST commit on the branch, before the
feature exists, with its regression test named for the revoked case — independently
cherry-pickable, and a revert of the gift can never quietly reopen an access hole (family
review, round 2).

**the last oracle is timing (lilith, comparison corrected by OMBB):** sealed must be
indistinguishable from **true not-found** on every channel — status, bytes, headers, AND
response time. the comparison that matters is sealed-404 (lookup-hit + liveness check)
vs unknown-token-404 (lookup-miss): both are dominated by the same single dynamo read and
the liveness check is pure in-memory arithmetic, so there is no clockable difference. the
serving path is irrelevant to this wall — it returns a distinguishable 200 by design.
named here so nobody re-derives it.

### 3. friend surface: the sealed view

a new pre-unlock state in `LinkPage`: the pea-soup attic, a wrapped pixel present (inline SVG
in the four-shade palette, like the landing key charm — no new asset pipeline), the link label
on the gift tag, and a countdown in the pixel type. reduced-motion gets a static wrapped
present with the date; the countdown text still updates (text change ≠ motion).

unwrap moment: when the countdown hits zero the client refetches; on `state: "active"` it
transitions into the normal shelf, whose existing boot/typewriter entrance IS the ceremony
beat (deliberate reuse, decided at the step-5 gate — no bespoke crossfade; the entrance
already honors `prefers-reduced-motion`). refetch retries with small backoff if the server still says sealed
(clock skew between lambda and client is real; remaining-seconds makes it rare, retry makes
it invisible).

### 4. admin: set it at create, edit it while sealed

- `CreateLinkBody` gains `unlock_at: Option<String>` (rfc3339 **with offset** — an absolute
  instant, resolved in ben's browser from a `datetime-local` input at write time; a bare date
  is how a gift opens at 7pm (lilith). the server stores only the instant). validation:
  must be in the future, must be strictly before the computed `expires_at` when both are set,
  bounded (≤ 370 days out) so a typo'd year can't seal a gift forever.
- links list shows sealed state + unlock time. two admin verbs, split on purpose (lilith:
  unsealing gets its own verb; a null that means "unseal" is a fat-finger away from a set):
  - `POST /admin/api/links/:token/unlock` `{unlock_at}` — **edit** the moment. rejects past
    instants and rejects null.
  - `DELETE /admin/api/links/:token/unlock` — **unseal** (remove the wrap entirely).
- immutability is enforced IN the storage condition, not by read-compare-write (TOCTOU at
  exactly the boundary that matters — lilith): both verbs run a scoped update conditioned on
  `attribute_exists(unlock_at) AND unlock_at > :now` (`:now` server-supplied). one condition
  yields all three rules atomically: editable while sealed, immutable once unlocked, and
  never addable to a link created unwrapped (re-wrapping a seen gift is a lie).
- **product constraint, in plain words (OMBB: say it, don't bury it in a condition
  expression): a seal is create-time-only.** a link born open can never gain a seal later,
  **because the server can't know whether a friend already looked** — once a link has been
  open for a second, its contents may have been seen, and wrapping seen contents is
  re-sealing by another door (lilith). the move is wrap-at-create, or recreate the link
  before sending. plans can move the moment or remove the wrap; they can never add one.
- **the complement property (lilith):** once `unlock_at` exists, the edit condition
  (`unlock_at > :now`) and the claim condition (`unlock_at <= :now`) are exact complements —
  no instant where both pass, none where neither does. tested from both sides at −1s /
  exactly / +1s on BOTH verbs; the exact row is where an off-by-one would surface as an
  overlap or a gap, and one-sided testing structurally cannot see it.
- admin wire note (OMBB): `handle_list_links` serializes `domain::Link` raw, so `unlock_at`
  reaches the admin list automatically (rfc3339-or-ABSENT — skip-on-None serde, the
  `thanked_at` combo; chosen at the step-5 gate so the stripped body can be key-free,
  not merely null-valued) — intentional, no admin view type to touch server-side.
- storage follows the enforcer-field rule OMBB set in the #69 review, sharpened type-level
  (lilith): `unlock_at` is authoritative in a **top-level numeric dynamo attribute** (epoch
  seconds, like `expires_at`; claim enforcement compares it numerically in the
  `claim_game` ConditionExpression). `schema::link_body` strips it via an exhaustive
  destructure of `Link`, so adding a field to `Link` without deciding its body fate is a
  compile error, and absent-attribute = unsealed is the ONLY representation (REMOVE on
  unseal, never null).

### 5. explicitly out of scope (YAGNI'd on purpose)

- occasion themes / wrapping-paper variants — one beautiful wrapped view first.
- push notification to ben at unwrap — the existing claim ping already covers the moment that
  matters; no scheduler infra for a nice-to-have.
- recurring occasions / birthday calendar — a different product.
- new infra: none. unlock is read-time gating; zero terraform beyond nothing.

## approaches considered

1. **read-time gating on the link record (chosen).** one new optional field + one refusal
   variant + view-layer withholding. no schedulers, no new tables, no eventbridge. the unlock
   "happens" the first time anyone looks after the instant — which is the only time unlock is
   observable anyway.
2. **scheduled mutation (eventbridge flips a `sealed` flag).** real infra, real IAM, real
   failure modes (missed fire = gift stuck sealed), and the flag can disagree with the
   timestamp. rejected: strictly worse than deriving state from time at read.
3. **client-side-only countdown (server serves everything, UI hides it).** rejected without
   mercy: the tease dies in devtools, and a crafted POST claims a sealed gift early. if the
   server doesn't enforce it, it isn't a feature, it's a decoration.

## testing (the placement-pin rule applies)

- domain: `can_claim` table tests for sealed×expired×revoked×exhausted orderings; serde
  round-trip + missing-field-is-None pin for `unlock_at`.
- public-api: sealed view withholds games/claims/note (assert the JSON *lacks* the fields —
  the test is the devtools friend); claim-while-sealed → 409; boundary test at exactly
  `now == unlock_at` (unlocked, not sealed — `>` not `>=`, matching `expires_at`'s `<=` edge).
- admin-api: create validation (past instant, unlock≥expiry, bound); scoped-update edit rules
  (editable sealed, immutable once unlocked, removable sealed).
- dynamo (moto): top-level attribute authority — body blob never carries `unlock_at`
  (the #69 strip test pattern), scoped update + read override round-trip.
- web: sealed render, countdown from remaining-seconds (fake timers), refetch-at-zero retry,
  reduced-motion paths, unwrap transition to normal shelf.

## family review (2026-08-05, shared channel)

all four calls blessed by lilith (msg 1534518820415475753), with sharpenings folded in above:
write-time timezone resolution to an absolute instant; type-level body strip; `no-store` on
sealed responses; claims-count-as-tease decided deliberately; immutability pushed into the
storage ConditionExpression (read-compare-write races at the boundary); unseal as its own
verb, past values rejected on set; ceiled remaining; refetch-never-self-unseal. OMBB's
sign-off lands at the plan gate (step 5).
