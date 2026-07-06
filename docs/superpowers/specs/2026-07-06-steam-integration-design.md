# Steam integration (OpenID sign-in + ownership badges) — design

**Status:** approved by Ben 2026-07-06 (brainstormed via Discord, all sections approved)
**Author:** kitten
**Depends on:** `2026-07-06-game-detail-modal-design.md` — the appid mapper + `Game.steam_app_id`
built there are what ownership matches against. Build order: modal spec first, then this.

## 1. Goal

Both Ben (admin) and gift-link friends can **sign in through Steam** (OpenID) so the app knows what
they already own:

- **Admin:** catalog games Ben owns on Steam get an `owned_by_ben` badge, and the self-claim confirm
  warns "you already own this on Steam — sure?" No separate library view (Ben, 2026-07-06).
- **Friends:** games they own get a "you own this" badge on the gift-link page (card + modal), so
  they don't burn a claim on a dupe. Purely advisory — never access control.

Credentials: Steam Web API key registered by Ben 2026-07-06 (domain bendobundles.com), stored per the
established secret path (SSM SecureString via deploy.tfvars; local pointer in kitten's
`~/.secrets/steam-web-api-key.env`). The key never reaches the browser.

## 2. steam-client crate (humble-client's twin)

```rust
pub struct SteamApiKey(String);            // redacted Debug, same discipline as SessionCookie
pub async fn verify_openid_assertion(params) -> Result<SteamId64, SteamError>
pub async fn get_owned_games(&key, steamid: &str) -> Result<Vec<u32>, SteamError>  // appids
pub async fn get_player_summary(&key, steamid: &str) -> Result<Persona, SteamError> // persona name/avatar
// plus the storefront reads the modal sync pass uses (appdetails, appreviews, applist)
```

- OpenID verify = server-side `check_authentication` round-trip to `steamcommunity.com/openid/login`
  (mode=check_authentication echo of the assertion params). **A steamid is only ever trusted after
  this round-trip** — never parsed off the return URL alone. Also assert the `claimed_id` pattern
  (`https://steamcommunity.com/openid/id/<64-bit>`), and that the assertion's `return_to` matches ours.
- `get_owned_games` = `IPlayerService/GetOwnedGames` (key-authorized, `include_played_free_games=1`);
  an empty/absent games array with a private profile is a **typed `PrivateLibrary`-shaped outcome**,
  not an error blob.
- Exhaustive `SteamError` enum, no `_` arm; wiremock fixtures for all of it.

## 3. The OpenID dance — one flow, two doors

"Sign in through Steam" button (official badge asset) → redirect to Steam's OpenID endpoint with
`return_to = https://bendobundles.com/api/steam/return?ctx=<path>` → Steam bounces back → **public-api
return endpoint** verifies the assertion via steam-client → redirects to `ctx` (the originating page:
`/l/<token>` or `/admin`) with the verified steamid in a fragment (`#steam=<id64>&persona=<name>`).

- `ctx` is validated against an allowlist of shapes (`/l/{token}`, `/admin`) — no open redirect.
- The fragment (not a query param) keeps the steamid out of server logs on the bounce.
- One return endpoint serves both surfaces (Ben's cross-surface requirement, 2026-07-06).

**Shared browser identity:** the SPA (both mounts are the same origin) stores the verified identity
under one localStorage key — `steam.identity = {steamid, persona, owned: [appids], fetched_at}`.
Sign in once in the admin, open a gift link in the same browser → already connected, badges just
light up. Per-browser only (phone ≠ desktop; one extra click there). "Not you? disconnect" clears
the key — keeps shared/batch links sane (Ben: links may be handed to a group, first-come-first-serve).

**Friend door (nothing persisted server-side):** after connect, the page calls
`GET /api/steam/owned/:steamid` — public-api proxies `get_owned_games` with the server-held key,
rate-limited like the link endpoints — and caches the appid set in `steam.identity`. Refresh on
reconnect or when `fetched_at` is older than a day. Badges compute client-side:
`game.steam_app_id ∈ owned`. The server stores **nothing** about friends — no steamid, no library.

**Admin door (one extra step):** the admin page, seeing a connected identity + a live admin session,
`POST /admin/api/steam/identity {steamid}` → persisted as a config item (`pk=CONFIG#STEAM`,
`sk=META`). That's what lets the backend act without Ben's browser: each `run_sync` refreshes Ben's
owned set server-side and stamps `owned_by_ben: bool` on matching games (top-level game field,
recomputed every sync). Disconnect in admin deletes the config item and the stamps clear next sync.

## 4. Surfaces

- **GameView (public) + AdminGame** gain `steam_app_id` (already public info) so badges can compute.
  AdminGame additionally carries `owned_by_ben`.
- **Friend page:** connect button in the link header when no identity; persona chip + disconnect when
  connected; "you own this" pill on owned cards and in the modal. A private-library connect shows
  "couldn't read your library — Steam privacy settings" once, politely.
- **Admin:** connect/disconnect in Ops; `owned_by_ben` badge in catalog; the self-claim confirm gains
  the already-owned warning line when applicable.

## 5. Honest caveats (accepted by Ben 2026-07-06)

1. Steam privacy: a friend with private "game details" reads as empty — detected and messaged, no
   workaround exists.
2. Friend library fetch happens at **connect time**, not sync time (unavoidable); it's one key'd
   Web-API call per connect, browser-cached after. The be-nice-on-syncs rule still fully governs the
   keyless storefront endpoints (modal spec §3).
3. Ownership is advisory. Spoofing a steamid earns badges, nothing else. No claim/auth decision ever
   reads it.

## 6. Rollout

1. steam-client crate (openid verify + owned games + persona), wiremock.
2. Return endpoint + `ctx` allowlist + owned-games proxy on public-api (rate limits + tests).
3. Friend SPA: connect flow, localStorage identity, badges, private-library messaging.
4. Admin: identity persist endpoint + config item, sync stamping `owned_by_ben`, catalog badge +
   self-claim warning.
5. Live check: Ben connects on both surfaces, one friend link exercised end-to-end.

## 7. Verification

Wiremock: openid check_authentication (valid, invalid, replayed), GetOwnedGames (open, private,
empty). Moto: config item lifecycle, sync stamping + un-stamping, proxy rate-limit behavior.
SPA: identity storage/disconnect, badge computation, cross-surface carry-over (connect in admin →
badges on a link page in the same browser). Security checks in review: no api key in any response,
no `ctx` open redirect, steamid only trusted post-verification.
