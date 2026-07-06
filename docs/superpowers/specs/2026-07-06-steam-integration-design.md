# Steam integration (OpenID sign-in + ownership badges) — design

**Status:** approved by Ben 2026-07-06; revised same day after spec review (B1, M1–M4 + minors
addressed)
**Author:** kitten
**Depends on:** `2026-07-06-game-detail-modal-design.md` — the appid mapper, `Game.steam_app_id`,
and the steam-client crate started there. Also `2026-07-06-self-claim-design.md` — the
"already owned" warning lands in the self-claim confirm, so that feature ships first.
Build order: self-claim → modal → this.

## 1. Goal

Both Ben (admin) and gift-link friends can **sign in through Steam** (OpenID) so the app knows what
they already own:

- **Admin:** catalog games Ben owns on Steam get an `owned_by_ben` badge, and the self-claim confirm
  warns "you already own this on Steam — sure?" No separate library view (Ben, 2026-07-06).
- **Friends:** games they own get a "you own this" badge on the gift-link page (card + modal), so
  they don't burn a claim on a dupe. Purely advisory — never access control.

**Why OpenID and not paste-your-profile:** a vanity-URL paste box (ResolveVanityURL) would deliver
the same advisory badge with far less machinery, and was proposed. Ben chose OpenID explicitly
(2026-07-06): one-click UX, correct-by-construction id, persona/avatar for free, and the same shiny
button on both surfaces. The verification apparatus below is the cost of that choice, accepted.

Credentials: Steam Web API key registered by Ben 2026-07-06 (domain bendobundles.com), stored per
the established secret path (SSM SecureString via deploy.tfvars). The key never reaches the browser.
**Assumption (M2, load-bearing):** the key is registered on **Ben's own Steam account** — the same
account as the persisted admin steamid. GetOwnedGames bypasses the "game details" privacy setting
only for the key owner's own library; if the key were registered elsewhere and Ben's privacy were
non-public, his library would read private-empty. **Confirmed by Ben 2026-07-06** (key made while
logged into his own account); revisit if the key is ever re-registered.

## 2. steam-client crate (extends the crate the modal spec starts)

```rust
pub struct SteamApiKey(String);        // redacted Debug, same discipline as SessionCookie
pub struct SteamId64(String);          // the newtype flows through every signature below
pub async fn verify_openid_assertion(params) -> Result<SteamId64, SteamError>
pub async fn get_owned_games(&key, steamid: &SteamId64) -> Result<OwnedGames, SteamError>
pub async fn get_player_summary(&key, steamid: &SteamId64) -> Result<Persona, SteamError>
pub async fn resolve_vanity(&key, name: &str) -> Result<SteamId64, SteamError>  // cheap, keep for tooling
```

Plain reqwest (no wreq fingerprint machinery — per the modal spec). Exhaustive `SteamError`, no `_`
arm; wiremock fixtures throughout.

- OpenID verify = server-side `check_authentication` round-trip (keyless) echoing the assertion
  params. A steamid is trusted only after: the round-trip returns `is_valid:true`, the `claimed_id`
  matches `https://steamcommunity.com/openid/id/<64-bit>`, and `openid.return_to` **exactly matches
  the URL of the request being handled** (the standard OpenID rule — with `ctx` embedded as a query
  param, exact-match is what makes tampering visible). Replay defense rides on Steam's own
  `response_nonce` single-use enforcement inside check_authentication.
- `get_owned_games` = `IPlayerService/GetOwnedGames` (keyed, `include_played_free_games=1`).
  **Private-vs-empty, pinned to response shape (M4):** `response.game_count` **absent** ⇒ private
  ("game details" hidden — typed `SteamError::PrivateLibrary` or an `OwnedGames::Private` variant);
  `game_count: 0` present ⇒ genuinely empty library. Do NOT infer privacy from
  GetPlayerSummaries' `communityvisibilitystate` — that reflects *profile* visibility, a different
  setting. Wiremock fixtures for both shapes.

## 3. The OpenID dance — one flow, two doors

"Sign in through Steam" (official badge asset) → redirect to Steam's OpenID endpoint with
`return_to = {BASE_URL}/api/steam/return?ctx=<path>` → Steam bounces back → the **public-api return
endpoint** verifies (steam-client) → on success, one keyed `get_player_summary` call → redirects to
`ctx` with `#steam=<id64>&persona=<url-encoded name>` (fragment, not query — stays out of server
logs; persona names are arbitrary unicode, URL-encoded).

- `ctx` is validated against the shape allowlist (`/l/{token}`, `/admin`) **twice**: at
  redirect-initiation (the SPA only ever emits its own path) and at return (no open redirect).
- `return_to` is built from the existing `BASE_URL` env var, so a dev-origin deployment works
  without code changes; the §6.5 live check remains the end-to-end receipt.
- **Failure contract (M3):** verify fails or steamcommunity times out → `302` to `ctx` with
  `#steam_error=verify_failed` / `#steam_error=steam_unreachable` (the SPA shows one polite line);
  `ctx` fails the allowlist → `302` to `/` with no fragment. The endpoint is unauthenticated and
  makes one keyless outbound call per hit (persona only after success) — accepted under the global
  stage throttle, same class as the link fetch.

**Shared browser identity:** both mounts (same origin) store the verified identity under one
localStorage key — `steam.identity = {steamid, persona, owned: [appids], fetched_at}`. Sign in once
in the admin, open a gift link in the same browser → already connected. Per-browser only (phone ≠
desktop). "Not you? disconnect" clears the key — batch/shared links stay sane.

**Friend door (nothing persisted server-side about friends, except the cache below):** after
connect, the page calls **`GET /api/l/:token/steam/owned/:steamid`** — **scoped under a live link
token**, exactly the modal detail endpoint's token-as-guard model. This is the B1 fix, and an
honest correction: the earlier draft said "rate-limited like the link endpoints," believing
per-token limiting existed — it does not (the only throttle is the global 25 rps / 50 burst API
Gateway stage limit, whose terraform comment admits per-token limiting is aspirational). An
unscoped `/api/steam/owned/:steamid` would have been an unauthenticated open proxy through Ben's
key — free library-enumeration for the internet, quota exhaustion in ~an hour at stage-limit rates.
Scoped: no live link token, byte-identical 404 (the existing no-oracle discipline); dead/exhausted
links refuse the same way the link fetch does.

**Server-side ownership cache:** `pk=STEAMOWN#{steamid}`, `sk=META` — owned appid set +
`fetched_at`, DDB TTL ~7 days. The proxy serves the cache when fresh (≤24h), fetches through the
key otherwise. Bounds repeat traffic from shared links and browser-cache clears; also what the
admin sync reads/writes for Ben's own set. Per-connect keyed-call budget: ≤2 (persona at return +
owned-games on cache miss).

**Admin door (one extra step):** the admin page, seeing a connected identity + a live admin
session, `POST /admin/api/steam/identity {steamid}` → persisted at `pk=CONFIG#STEAM`, `sk=META`.
**`CONFIG#` is a net-new pk family** (today's table has only GAME/LINK/SESSION/SYNC) — new schema
helpers + a fulfillment read path come with it; rollout step 4 absorbs that. Each `run_sync` then
refreshes Ben's owned set server-side and stamps `owned_by_ben: bool` on matching games. The admin
browser's own fetches go through session-guarded `GET /admin/api/steam/owned/:steamid` (same cache).
Disconnect deletes the config item; stamps clear on the next successful refresh.

**`owned_by_ben` failure semantics (M1 — the modal spec's "never fail the sync over marketing copy"
twin):** stamps are recomputed **only from a successful, non-private GetOwnedGames response**. On
transient failure, key trouble, or a private/empty-shaped result (see M2/M4): keep the prior
stamps, log one line (`steam owned refresh skipped: <reason>`), never fail the sync. A
private-shaped result for Ben's own steamid additionally pings once (it means the M2 assumption
broke — key re-registered or privacy changed), since silently-stale warnings are worse than noise.

## 4. Surfaces

- **GameView (public) + AdminGame** gain `steam_app_id` (already public info) so badges compute
  client-side. AdminGame additionally carries `owned_by_ben`.
- **Friend page:** connect button in the link header; persona chip + disconnect when connected;
  "you own this" pill on owned cards and in the modal. A private-library result shows "couldn't
  read your library — check Steam's *game details* privacy setting" once, politely.
- **Admin:** connect/disconnect in Ops; `owned_by_ben` badge in catalog; the self-claim confirm
  (the shared arm/confirm component) gains the already-owned warning line.

## 5. Honest caveats (accepted by Ben 2026-07-06)

1. Steam privacy: a friend with private "game details" reads as empty — detected per the M4 shape
   rule and messaged; no workaround exists.
2. Friend-side keyed traffic happens at **connect time and on cache expiry** (≤2 keyed calls per
   connect, ≤1/day per steamid after — server cache 24h, browser cache alongside), not at sync
   time. The be-nice-on-syncs rule still fully governs the keyless storefront endpoints (modal
   spec §3).
3. Ownership is advisory. Spoofing a steamid earns badges, nothing else. No claim/auth decision
   ever reads it. (The OpenID verification exists for correctness-of-UX, not security — see §1.)

## 6. Rollout

1. steam-client additions: openid verify + owned-games (both response shapes) + persona + vanity,
   wiremock.
2. Return endpoint (ctx allowlist both ends, failure contract, BASE_URL-derived return_to) +
   token-scoped owned proxy + STEAMOWN cache on public-api (moto + no-oracle tests).
3. Friend SPA: connect flow, localStorage identity, badges, private-library + steam_error
   messaging.
4. Admin: CONFIG# schema helpers, identity persist/delete endpoints, session-guarded owned proxy,
   sync stamping with M1 semantics, catalog badge + self-claim warning.
5. Live check: Ben connects on both surfaces (confirming the M2 key-account assumption), one friend
   link exercised end-to-end incl. a private-profile friend if available.

## 7. Verification

Wiremock: check_authentication (valid, invalid, timeout), GetOwnedGames (public-with-games,
`game_count:0` empty, absent-game_count private), persona. Moto: STEAMOWN cache
(fresh/stale/miss), CONFIG#STEAM lifecycle, sync stamping — success recomputes, failure keeps prior
stamps, ben-private pings once; token-scoped proxy 404s without a live link (byte-identical),
session guard on the admin variant. SPA: identity storage/disconnect, badge computation,
steam_error fragments, cross-surface carry-over (connect in admin → badges on a link page in the
same browser). Security checks in review: no api key in any response or log, no ctx open redirect,
exact return_to match, steamid trusted only post-verification.
