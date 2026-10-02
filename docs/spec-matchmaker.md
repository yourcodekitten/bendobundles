# The Matchmaker 💘

*Ben opens the Friends page. Next to Sarah's name it says: 4 games from your attic are on her wishlist.
One of them has been on it since 2022. He ticks three, presses one button, and a curated link with
those three is ready to send. Today he would have had to guess.*

**Status:** spec · 2026-10-02 · author: code kitten
**Surface:** admin only, `/admin/friends`. Nothing on the friend side changes.
**Sibling of:** the wishing well (#259). The wishing well lights up the friend's side after Ben has
already picked the games. The matchmaker helps Ben pick them in the first place.

---

## 1. The gap, measured 2026-10-02 at `38e1748`

- `domain::Friend` is `{id, name, shelf_token, created_at}`. **No Steam identity** is stored for a
  friend anywhere. The only Steam identity at rest is Ben's own (`CONFIG#STEAM`).
- `steam-client` already has both verbs this needs: `get_wishlist` (keyless) and
  `get_owned_games` (keyed). The admin API already proxies owned games
  (`GET /admin/api/steam/owned/{steamid}`, via `Store::cached_owned_or_fetch`, 24h fresh).
- The admin catalog (`GET /admin/api/catalog`) already ships `steam_app_id`, `status`, `giftable` and
  `hidden` for every game, which is everything needed to decide whether a game can still be given away.
- Catalog → Links already hands a selection over via router state (`navigate('/admin/links',
  { state: { picked } })`, `Catalog.tsx`). The curated-link create flow accepts it.
- PRODUCT.md: *"ben forgets his bundles exist 95% of the time"* and *"an unclaimed game is a gift
  nobody got to open."* The 2026-09-28 census found **59 unclaimed slots across 11 live links, with
  no claim since 2026-07-14.** Links built from games a friend already wants should do better.

## 2. What Ben sees

1. **A Steam field per friend.** In the friend's row, a small `steam profile` input. It accepts:
   - a 17-digit SteamID64,
   - `https://steamcommunity.com/profiles/<id64>`,
   - `https://steamcommunity.com/id/<vanity>` (resolved server-side, §3.3).
   Saved, it shows as the friend's Steam persona name with a `change` / `forget` control.
2. **The match line.** For a friend with a Steam profile, the row shows one line:
   `💘 4 from your attic are on their wishlist` (singular: `1 … is`). If nothing matches, it shows
   nothing at all. As with the wishing well, an empty wishlist and a private one look identical on the
   wire, so the UI never says "empty" or "private".
3. **The match panel** (the line expands). One card per matching game: cover art, title, bundle,
   `on their wishlist since mar 2022` (reusing `formatSince`), and a checkbox. Order: wishlist
   **priority** first (the friend's own ranking), then `date_added` oldest first, because the longest
   wait is the best surprise.
4. **Owned games are removed, not just marked.** A game the friend already owns on Steam is never
   suggested. When the friend's library is private, the panel shows a quiet note: `can't see their
   library — they might own some of these`. (Here "private" can be measured: the owned endpoint
   returns `game_count` absent, which `OwnedGames::Private` already models.)
5. **"cut a link with these".** It navigates to `/admin/links` with `state.picked` (the existing
   handoff) plus `state.friendId`. The Links page preselects the curated mode, the picks, and the
   friend to attach once the link exists. Ben still names, notes and creates the link himself. The
   matchmaker **never creates a link on its own.**

## 3. The API

### 3.1 Friend Steam identity
`POST /admin/api/friends/{id}` grows a fourth mutually exclusive field: `steam: "<input>" | null`.
- The input is parsed (§3.3) into an id64. Invalid input ⇒ `422` with a message saying which forms are
  accepted.
- `null` forgets the identity (REMOVEs the attribute; nothing left at rest).
- It is stored as a top-level attribute `steam_id64` on `FRIEND#<id>/META`, and `Friend` gains
  `#[serde(default)] steam_id64: Option<String>`. Older records read back as `None`.
- The existing "exactly one field per request" rule stays and simply counts four fields instead of three.
- **Lifetime:** the id64 lives on the friend's own `META` item. Measured at `38e1748`, **there is no
  friend-delete route** (the friend routes are `POST`+`GET /admin/api/friends` and `POST /admin/api/friends/{id}`;
  revoke keeps the record). The only way to remove the id64 is therefore `steam: null`. If a delete route
  is ever added, deleting the item removes the id64 with it, because there is no second copy (Lilith's
  condition, family review). Revoking a friend's shelf does **not** forget their Steam id. Ben forgets it explicitly.

### 3.2 Wishlist proxy
`GET /admin/api/steam/wishlist/{steamid}`. It mirrors the admin owned proxy's preamble exactly:
steam client absent ⇒ `503`, invalid id64 ⇒ `400`, upstream error ⇒ `503`. On success:
`{ "items": [{appid, priority, date_added}] }`.
**Nothing is stored.** The wishing well's rule carries over: the wishlist is read live every time and
never written. The response is `Cache-Control: private, max-age=300`.
⚠️ `WishItem` currently has `appid` and `date_added` only. **`priority` must be added**, in
the steam-client struct and in its lenient decoding. (#260 is the open issue about one bad item
emptying the whole list. Fixing it is **in scope here**, because the matchmaker reads the same list.)

### 3.3 Profile input parsing / vanity resolution
- id64 and `/profiles/<id64>`: parsed locally, no network.
- `/id/<vanity>`: `ISteamUser/ResolveVanityURL/v0001/?vanityurl=<name>&key=…` (**keyed**). This is a
  **new steam-client verb**, so the domain verb-count change detector moves 11 → 12 on purpose, and the
  new verb goes through `net()` like every other.
- `success != 1` ⇒ `422 "couldn't find that steam profile"`.

### 3.4 The match itself is client-side
The admin page already has the full catalog. The match is a pure function in `web/src/matchmaker.ts`:
`match(catalog, wishItems, owned | 'private') → Match[]`
- candidates: catalog rows with `status === 'available' && giftable && !hidden && steam_app_id != null`
  (the same predicate as `Game::is_listable`, plus a mapped appid);
- keep the rows whose appid is in the wishlist; drop them if the appid is in `owned`;
- **dedupe by `steam_app_id`**, not title (OMBB, family review): one appid can sit under two catalog
  titles across bundles, and a title dedupe keeps both. Inside an appid group the tiebreak is the copy
  with a bundle name, then the title, then the lowest id, so the result is deterministic;
- sort: `priority` ascending, then `date_added` ascending, then title.
It is pure, so every rule above gets a unit test.

## 4. What this deliberately does NOT do
- **No cross-friend board** ("who wants this game?") in v1. The data would support it, but it means one
  wishlist call per friend on every page load. That is a v2 question, raised below.
- **No automatic link creation and no notifications to friends.** The matchmaker only suggests. Ben
  still does the giving.
- **The friend side is untouched.** Ben recording a friend's Steam id changes nothing on that friend's shelf or links.

## 5. Failure modes
- Steam down or rate-limited ⇒ that friend shows no match line. No error is printed in the row,
  because a missing suggestion costs nothing. The panel, if it's open, says `steam isn't answering right now`.
- A friend's profile is deleted or the vanity changes ⇒ the stored id64 stays valid (an id64 never
  changes). Vanity is resolved once, at save time.
- Many friends ⇒ wishlist calls are made **lazily**: one per friend row once it scrolls into view, at
  most 4 at a time. They are not fanned out on page load.

## 6. Family decisions (2026-10-02, Lilith; OMBB pending)
- **D1, store the id64 (was Q1).** The wishing well's nothing-at-rest rule was about the *wishlist*,
  which is still never stored. An id64 is a public identifier Ben typed into his own address book. If he
  had to paste it every session he would never use the feature.
- **D2, resolve vanity URLs (was Q2).** `/id/<name>` is the form Ben will actually copy out of Steam.
  It is resolved once at save time, so the cost is one keyed call per friend, ever. The verb count moving
  11 → 12 is the detector working as intended.
- **D3, the census decides any board (was Q3).** **No v2 commitment in this spec.** The baseline is the
  2026-09-28 census (59 unclaimed slots, no claim since 2026-07-14). After v1 ships, check whether links
  cut from wishlist matches get claimed at a better rate. If they do, a cross-friend board has earned its
  N calls. If they don't, the bottleneck isn't picking.

## 6.1 Unresolved, must be decided before the plan (OMBB, 2026-10-02, verified at `38e1748`)
**Q1 was wider than the id64.** The owned-games exclusion goes through the admin owned proxy, which calls
`Store::cached_owned_or_fetch`. That **writes `STEAMOWN#<id64>`**, which holds the friend's **whole owned
library**, with a 7-day TTL (`put_steam_owned` → `schema::steam_owned_item`). `steam: null` does not touch
it. So "nothing left at rest" and a test asserting only "no `STEAMWISH#` write" are both too narrow: the IAM
capture would show that put. Pick one and write it here:
- **(a)** the TTL'd cache is acceptable, stated as such, with the 7-day bound named; or
- **(b)** `steam: null` also deletes `STEAMOWN#<id64>`. This is a new delete path, and it has to be added to the IAM capture.

## 6.2 The yardstick, pre-registered BEFORE any data (OMBB)
D3 is only honest if the measure is fixed before anyone sees results. **Measure:** the claim rate of links
cut through the matchmaker (claims / slots, over the 30 days after creation), against the 2026-09-28
baseline (59 unclaimed slots across 11 live links, no claim since 2026-07-14). ⚠️ This needs a way to tell
a matchmaker-cut link from any other link. Today nothing on `Link` records how it was made, so the plan
must add a provenance marker or the measure cannot be taken.

## 7. Testing
- `matchmaker.ts`: predicate, owned exclusion, `private` passthrough, title dedupe determinism, sort order.
- steam-client: `priority` decoded; a malformed item drops only itself (#260); vanity resolve success,
  `success: 42`, network error.
- admin-api: the `POST /admin/api/friends/{id}` four-way exclusivity (the route is `POST`; its handler is named `handle_patch_friend`), the `steam` set and forget round-trip, the wishlist proxy
  preamble (503 / 400 / 200), and a test that **no `STEAMWISH#`-style write ever happens** (asserted by
  the IAM capture, the same way the wishing well asserted it).
- admin-api: `steam: null` REMOVEs `steam_id64` (asserted on the raw item, not only the read-back), and a revoke leaves it in place.
- web: the Friends row hides the line when there are zero matches, the panel → Links handoff carries
  `picked` and `friendId`, and Links preselects curated mode with the friend attached after create.
