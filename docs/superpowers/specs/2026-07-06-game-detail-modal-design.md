# Game detail modal + Steam enrichment cache — design

**Status:** approved by Ben 2026-07-06 (brainstormed via Discord, all sections approved)
**Author:** kitten
**Depends on:** the sync walk in fulfillment (`run_sync`), the existing GameGrid/Catalog SPA surfaces.
**Depended on by:** `2026-07-06-steam-integration-design.md` (the appid mapper built here is the
foundation for ownership matching there). Build order: this before steam-integration.

## 1. Goal

Clicking a game — on the friend gift-link page **and** the admin catalog — opens a modal like the
Humble/Steam store pages: trailer video, Steam review summaries (**recent + overall**, English
reviews), developer, publisher, tags/genres, release date, short description. Content comes from
Steam's public storefront data, fetched **only during syncs** (Ben's be-nice-to-Steam rule,
2026-07-06) and cached in DynamoDB; request-time reads never touch Steam.

## 2. The appid mapper (the real work, shared with steam-integration)

Humble gives `machine_name` + title; Steam content is keyed by **appid**. A sync pass resolves
`Game.steam_app_id: Option<u32>` (new domain field) for steam-keytype games, in resolution order:

1. **Humble's own tpk data** — order tpks carry a `steam_app_id` field (proven in the choice design
   doc §2, live HAR). When present, free and authoritative.
2. **Humble store link** — subproduct/order payloads often carry a `store.steampowered.com/app/<id>`
   URL; parse the id out.
3. **Exact title match** — against Steam's app list (`ISteamApps/GetAppList` cached per sync) after
   light normalization (case, trademark glyphs). Exact-only: fuzzy matching mislabels games, and a
   wrong trailer is worse than none.
4. **Admin override** — `POST /admin/api/games/:id/steam-app-id {app_id | null}` + a small "set appid"
   affordance in the catalog for the misses (old bundle titles will miss; that's expected).

Manual overrides win over auto-resolution and are never clobbered by later syncs (an
`appid_source: manual|humble|title` marker on the game enforces this). Non-steam key types skip the
mapper entirely.

## 3. Enrichment cache

New single-table item per appid — `pk=STEAMAPP#{app_id}`, `sk=META` — holding:

- from `store.steampowered.com/api/appdetails?appids=<id>` (keyless storefront): trailer URL
  (mp4 + webm), header image, developers, publishers, genres + tags, release date, short description.
- from `store.steampowered.com/appreviews/<id>?json=1&language=english`: review summary **twice** —
  recent (`num_per_page=0` default window) and overall — score description ("Very Positive"),
  percent-positive, review counts.
- `fetched_at` epoch.

**Refresh policy (be-nice rule):** enrichment runs as a `run_sync` pass, after game discovery. Only
appids that are new or stale (`fetched_at` older than 7 days) are fetched, serially, politely spaced
(≥ ~350ms between storefront calls). A 700-game catalog therefore costs ~0 steam calls on a normal
sync and a slow, polite trickle on the first one. Per-appid fetch failures are logged and skipped —
never fail the sync over marketing copy. Region/currency quirks: request `cc=us&l=english`.

Multiple humble games mapping to one appid (re-bundled titles) share one STEAMAPP item — the cache is
per-app, not per-game.

## 4. API

- Friend: `GET /api/l/:token/games/:id/detail` — token must be a live link (same validation as the
  link fetch, rate-limited the same way); returns the game's public fields + the STEAMAPP blob.
- Admin: `GET /admin/api/games/:id/detail` — session-guarded, same shape.
- Both are pure cache reads. No Steam call, no secrets in the response (`gamekey`/`machine_name`/
  `keyindex` stay server-side exactly as today).

## 5. Web

One `GameDetailModal` component, two mounts (friend GameGrid card click, admin Catalog row click):

- trailer up top, **click-to-play** (no autoplay), poster = header image; artwork fallback when no
  trailer.
- title, developer / publisher / release date line; two review badges (recent + overall) with
  count + percent tooltips; tag chips; short description.
- **friend mount:** the claim button lives in the modal footer (browse → claim in one flow), wiring
  into the existing ClaimDialog; disabled states follow the existing grid rules.
- **admin mount:** status badge + the self-claim action (per the self-claim design) in the footer.
- **thin fallback** for non-steam or unmapped games: artwork, bundle, key type, "no steam page for
  this one" — the modal never looks broken, just quieter.
- Escape/backdrop closes; no data fetch until opened (lazy per-game detail call, cached in component
  state for the session).

## 6. Rollout

1. Domain field + mapper pass (tiers 1–3) + moto tests; log per-sync mapping coverage
   (mapped/unmapped counts).
2. Admin override endpoint + catalog affordance.
3. Enrichment pass + STEAMAPP items (wiremock fixtures for appdetails/appreviews; staleness + spacing
   tested with a fake clock).
4. Detail endpoints (public + admin).
5. `GameDetailModal` + both mounts.

## 7. Verification

Wiremock fixtures for appdetails (with + without movies), appreviews (recent vs overall), GetAppList.
Moto tests: mapper resolution order incl. manual-override wins + no-clobber, staleness window,
per-app failure isolation. SPA: modal renders full + thin variants; claim-from-modal path exercises
the existing ClaimDialog states. Live check after first deployed sync: spot-read a handful of
STEAMAPP items + Ben eyeballs the modal on both surfaces.
