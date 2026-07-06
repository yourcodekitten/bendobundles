# bendobundles — design spec

**date:** 2026-07-02 · **author:** code kitten · **approved by:** ben (discord, 2026-07-02)
**domain:** bendobundles.com (ben purchasing) · **repo:** `bendoerr/bendobundles`

## 1. purpose

ben has ~15 years of humble bundle purchases full of unclaimed game keys, browsable today only
bundle-by-bundle on humblebundle.com. bendobundles is a small web app that:

1. lists his ENTIRE humble library in one place (keys + DRM-free + ebooks + everything), and
2. lets him hand friends invite links that grant N game claims — a friend clicks claim and
   instantly receives a humble **gift link** for that game, redeemable into their own account.

## 2. decision log (all ben-approved, 2026-07-02)

| decision | choice |
|---|---|
| claim mechanic | **full magic** — gift link generated live at claim time via ben's humble session |
| friend auth | **none. just the link.** invite URL is a bearer capability carrying N tracked claims |
| admin auth | single admin password (argon2 in SSM, session cookie, rate-limited). verbatim: "fuck cognito." passkeys = fun later upgrade |
| catalog scope | friends see giftable keys only; ben's admin ingests EVERYTHING (keys, DRM-free, ebooks) with non-giftable badged yours-only |
| friend-view default | default-open: whole unclaimed giftable library minus per-game hidden toggle ben controls |
| hosting | serverless AWS — lambda + dynamodb + s3/cloudfront + apigateway, scale-to-zero, built from `bendoerr-terraform-modules/*` legos |
| backend language | **rust** (cargo-lambda, aws-sdk-rust, axum/lambda_http). ben enjoys reviewing rust. fallback was go |
| frontend | typescript SPA (vite + react), static on s3/cloudfront |
| architecture | **split by trust boundary** (option B): 3 lambdas, only `fulfillment` can read the humble session |
| humble access | community-documented unofficial API using ben's session cookie; gift-link mechanic is the fulfillment rail |

## 3. architecture

```
friend/ben browser
   │
cloudfront ── s3 (TS SPA: /l/<token> friend view, /admin ben view)
   │
apigateway
   ├── public-api lambda   (catalog browse, claim intake; NO access to humble secret — IAM-enforced)
   └── admin-api  lambda   (login, invite-link CRUD, hidden toggles, cookie paste, sync-now)
        │
fulfillment lambda ← eventbridge (daily sync) + invoked by public-api (narrow contract) + admin-api (sync-now, cookie validation)
   │  the ONLY component that talks to humblebundle.com
   │  the ONLY role that can read the session secret
   ▼
SSM SecureString (humble session cookie, admin password hash)      dynamodb (single table, on-demand)
```

- one rust cargo workspace produces all three lambdas; shared crates: `domain`, `humble-client`,
  `dynamo` (storage).
- public-api → fulfillment is a synchronous lambda invoke with a minimal payload
  (`{claim_id, key identifiers}`) and minimal response (`{gift_url}` or typed error). IAM scopes
  exactly this invoke.
- idle cost ≈ $0 (on-demand dynamo, no NAT, no always-on compute). SSM standard params are free.

## 4. data model — dynamodb single table

three item families:

**GAME** — one item per humble *key/entitlement* (per-key, NOT per-title; 15 years = duplicate
copies of the same title, each independently claimable; UI groups by title with copy counts).
fields: title, bundle name, humble order id ("gamekey") + key id (tpkd), key type
(steam/gog/origin/drm-free/ebook/…), `giftable` flag, `hidden` toggle (ben), status:
`available` | `pending` | `gifted` | `ben-redeemed` | `expired`, artwork url, timestamps.

**LINK** — one item per invite link. fields: high-entropy random token (≥128-bit, the bearer
capability), human label ("dave bday"), claims_allowed, claims_used, optional expiry, revoked
flag, created_at.

**CLAIM** — the receipt. fields: claim id, link token, game id, timestamp, **gift_url**, state
(`pending` | `fulfilled` | `compensated`). a friend's link page lists their claims + gift URLs
forever (losing the tab loses nothing).

**concurrency:** claim intake = one `TransactWriteItems`: GAME `available→pending` (conditional
on status) + LINK `claims_used += 1` (conditional on `< claims_allowed`, not revoked, not
expired). first-come-first-served falls out of the transaction; a lost race is a clean, friendly
failure with nothing burned.

## 5. claim flow

**invariant: a humble key burns exactly once, and a burned key's gift URL is never lost.**

happy path:
1. friend opens `/l/<token>` → public-api validates link → catalog of available, giftable,
   non-hidden games.
2. claim click → the transaction above → GAME pending, slot consumed.
3. public-api invokes fulfillment → fulfillment calls humble redeem-as-gift → receives one-time
   gift URL.
4. fulfillment **writes gift_url to the CLAIM record first** (durable the instant it exists),
   then flips GAME→`gifted`, then returns the URL to the friend's browser.

failure handling:
- **clean humble failure** (5xx/timeout-before-send/dead cookie): compensate — GAME→`available`,
  LINK counter decremented, claim `compensated`; friend sees "humble hiccuped, try again."
- **ambiguous failure** (timeout where the key MIGHT have burned): NO blind retry. claim parks
  `pending`; fulfillment's next scheduled pass re-checks the key's true state against humble and
  either completes (recovers the gift URL) or compensates. no double-burn, ever.
- **dead session cookie**: flag in dynamo → admin banner + discord webhook ping to ben; claims
  fail politely until a fresh cookie is pasted.
- friend-facing errors never leak anything about ben's humble account.

## 6. edge cases (decided)

- **ben redeems on humble directly:** sync treats humble as source of truth → `ben-redeemed`,
  removed from friend view. friend-claim in the gap → gift call fails "already redeemed" →
  compensate + record fixed. self-healing.
- **duplicate copies:** per-key GAME items; UI groups by title with copy count; N friends can
  claim N copies.
- **dead/expired keys** (decade-old bundles): surfaced by sync or failed gift calls → `expired`,
  hidden from friends, visible-badged in admin.
- **humble choice/monthly:** chosen keys ingest normally. unchosen choice months are badged
  "needs choosing on humble" in admin only; not claimable; out of v1 claim scope.
- **leaked/forwarded links:** accepted bearer-capability risk. mitigations: token entropy,
  per-link expiry + revoke, per-link claim audit in admin, rate-limited lookups.
- **gifted-but-never-redeemed:** gift URL stays on the friend's claim page; admin redemption
  badge only if the API exposes state (nice-to-have, not v1).
- **region-locked steam keys:** one honest disclaimer line in friend UI.
- **humble bot-detection:** paced + jittered sync, one-shot human-triggered claim calls;
  captcha/blocks degrade to cookie-dead-style politeness + ben ping.

## 7. no-token & token-state behavior

- **bare root / unknown token:** identical cute static landing page; ZERO catalog data; nothing
  enumerable. deliberately indistinguishable so token existence can't be probed.
- **revoked/expired link:** claim history stays readable (they own those gift URLs); claiming
  disabled — "this invite isn't active anymore, bug ben."
- **exhausted link:** browsing + history work; claim buttons disabled ("you've used all your
  claims").
- token lookups are rate-limited.

## 8. admin surface (`/admin`, password login)

- full catalog: everything ingested, badges (giftable / yours-only / expired / needs-choosing /
  hidden), per-game hidden toggle, search/filter, group-by-title and by-bundle views.
- invite links: create (label, N claims, optional expiry), revoke, per-link claim audit.
- cookie paste-box → SSM SecureString → fulfillment immediately validates and reports pass/fail
  inline.
- sync status + "sync now" button (async fulfillment invoke).

## 9. sync & cookie lifecycle

- eventbridge daily + manual sync-now. paced + jittered requests.
- first sync backfills ~15 years of orders (paginated; well inside lambda's 15-min ceiling;
  incremental thereafter).
- sync upserts GAME items, reconciles statuses (humble = source of truth), flags dead cookie,
  completes/compensates parked `pending` claims.
- cookie death → discord webhook + admin banner.

## 10. frontend

vite + react + typescript, static build to s3, cloudfront in front (also fronting apigateway).
- friend view: title-grouped grid with copy counts, search/filter, claim button, "your gifts"
  history with gift URLs.
- admin view: section 8.
- region-lock disclaimer line in friend view.

## 11. testing

- **humble-client gets the heaviest coverage** (it wraps an unofficial API that can shift):
  wiremock-based tests against recorded fixtures for every endpoint + error shape. no live
  humble in CI, ever.
- claim-transaction race tests against dynamodb-local (two claimers, one key; exhausted link;
  revoked link; compensation paths).
- clippy + rustfmt + cargo test gates; TS lint + typecheck; actionlint.
- e2e against real humble = manual, ben-triggered, post-deploy (his session, his call).

## 12. repo layout (`bendoerr/bendobundles`)

```
crates/
  domain/          # types, statuses, invariants
  humble-client/   # unofficial API wrapper (reqwest + serde)
  dynamo/          # storage layer
  public-api/      # lambda: friend surface
  admin-api/       # lambda: ben surface
  fulfillment/     # lambda: sole humble-toucher
web/               # vite + react + TS SPA
terraform/         # consumes bendoerr-terraform-modules/* legos
docs/superpowers/specs/   # this doc
.github/workflows/ # CI: build lambda artifacts, tests, lint
```

deployment v1: CI builds artifacts; ben runs `terraform apply` locally (his AWS creds stay his).

## 13. out of scope (v1) / later

- passkeys/webauthn for admin (fun upgrade)
- per-link catalog curation (layer on top of hidden-toggle model)
- gift-redemption-state badges (if API exposes it)
- adding labeling for whether friends actually played anything (joke. unless?)

## 14. risks (accepted, eyes open)

- **unofficial API:** humble can change/break it anytime; ToS gray-area for automation. paced,
  low-volume, personal-scale use. heaviest test fixtures live here to detect drift fast.
- **session cookie auth:** expires; requires ben re-paste. mitigated by validation-on-paste,
  death detection, discord ping.
- **gift links are single-use + irrevocable:** the exactly-once invariant + park-and-reconcile
  design exists precisely for this.
