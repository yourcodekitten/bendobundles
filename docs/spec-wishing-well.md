# The Wishing Well ⭐

*You open a gift link and there are forty-one games on the shelf. One of them has been on your Steam
wishlist since March 2023. Right now nothing on the page knows that, so you have to find it yourself.*

**Status:** spec · 2026-09-30 · author: code kitten
**Surface:** friend side, `/l/:token` (the gift-link page). Nothing on the admin side changes.
**Sibling of:** the "you own this" pill (`GameGrid.tsx`), which already uses the same Steam connect.
Owned tells you what to skip. This tells you what to take.

---

## 1. The gap, measured 2026-09-30

- `git grep -il wishlist -- crates web/src` ⇒ **0 files**. Nothing in the product reads a wishlist.
- The friend page already has a Steam OpenID connect, a token-scoped owned-games proxy
  (`handle_steam_owned_proxy`, `public-api/src/lib.rs`), a shared cache core
  (`Store::cached_owned_or_fetch`, `STEAMOWN#<id>`, 24h fresh, 7d TTL) and a localStorage identity
  (`steamIdentity.ts`). **The connect already happens. We only need to ask Steam one more question.**
- Production (from the 2026-09-28 census): **11 live links holding 59 unclaimed slots, with no claim
  since 2026-07-14.** Friends open a shelf, get overwhelmed by the number of choices, and leave. A
  wishlist match turns "browse 41 games" into "here's the one you already wanted".

### 1.1 The Steam endpoint, measured rather than assumed

`GET https://api.steampowered.com/IWishlistService/GetWishlist/v1/?steamid=<id64>`

- **Keyless.** Measured with no `key` param ⇒ `200`. We will **not** send the Web API key: it is not
  needed, and a secret on a request that does not need it is only risk.
- 40 real id64s from a public Steam group ⇒ **17 returned items, 23 returned `{"response":{}}`**.
- Item shape: `{"appid":3180380,"priority":0,"date_added":1790432023}` (epoch seconds).
- 🔴 **A private wishlist and an empty wishlist return the SAME bytes** (`{"response":{}}`). The
  product cannot tell them apart, so **no copy may ever say "your wishlist is empty"** or "your
  wishlist is private". An empty response just means nothing is marked, and the page says nothing
  about it.

## 2. What the friend sees

1. **The star.** Every card whose `steam_app_id` is on the friend's wishlist gets a `⭐ on your
   wishlist` pill, in the same chip row as the owned pill. Hovering or focusing it shows the date:
   `since mar 2023`, taken from `date_added`. The card's accessible name includes the same text.
2. **The float (open-shelf links only).** In a shuffled link, starred games move to the front. **The
   shuffle still happens inside each group:** starred games are shuffled among themselves, then the
   rest. The per-visit rank lock (`shuffleRanksRef`) still holds, so a claim refresh never
   rearranges the shelf.
3. **Curated links keep Ben's order.** A curated link's order is the order Ben picked (spec §5 of the
   curated work), so starred cards are marked but **never moved**.
4. **The count line (open AND curated links).** When at least one game matches, one line appears above the grid:
   `⭐ 3 of these are on your wishlist` (singular form: `1 of these is`). When nothing matches, no
   line appears. Silence is the correct output for "no match", and also for "can't see".
5. **Precedence.** If a game is both owned and wishlisted (Steam normally removes a game from the
   wishlist when you buy it, but stale data happens), **owned wins**: no star, the owned pill shows.
   A `gone` (ghost) card in a curated link never gets a star.

## 3. The API

`GET /api/l/{token}/steam/wishlist/{steamid}`. It mirrors the owned proxy in **every** preamble step,
in the same order:

1. Steam client not configured ⇒ `503`.
2. Resolve the token **first**. Unknown ⇒ the byte-identical `link_not_found_response()`; store error
   ⇒ `500 try again`.
3. Liveness via `link.can_claim(now)` ⇒ `409` with the same four messages.
4. Validate id64 **after** 2–3 (no oracle upgrade) ⇒ `400`.
5. Fetch from Steam (§4). There is **no cache**: every call is live.

**Response: `{"items":[{"appid":N,"added":EPOCH}, ...]}`, filtered to appids that appear among the
link's own games.** The server intersects before it answers, for two reasons:
- **Exposure.** The token proxy never serves anyone's full wishlist, only the overlap with the shelf
  they are looking at. (The data is public on Steam anyway, but we should not become a convenient
  mirror of it.)
- **Size.** The payload is bounded by the shelf, not by someone's 2,000-item wishlist.

A hidden or empty wishlist ⇒ `{"items":[]}`. There is no `private` flag, because we cannot measure one
(§1.1). Steam unavailable ⇒ `503`, which the client treats as "no stars" and never as an error banner.
The response gets `Cache-Control: private, max-age=3600`, so it lives only in the friend's own browser (§4).

## 4. Nothing at rest — no server cache (revised 2026-09-30 after OMBB's review)

The first draft cached the **full** wishlist at `STEAMWISH#<id64>` for 7 days. OMBB objected: "public"
is only true **at fetch time**. If a friend makes their wishlist private tomorrow, we would still be
holding all of it, keyed to their Steam id. **That is right, and the fix goes further than either of
his options: no server-side record at all.**

- The endpoint is keyless and cheap, and each page load calls it once. The owned cache exists because
  `GetOwnedGames` spends **our API key's** quota. The wishlist call spends nothing of ours.
- **At-rest contents, stated so the plan can be checked against it: the server stores NOTHING about
  a wishlist.** No dynamo item and no log line carrying appids. The browser holds only the
  **overlap** (§3), in React state, for the life of the page. It is **not** written to localStorage.
- Repeat loads are absorbed by `Cache-Control: private, max-age=3600` on the proxy response, which
  lives in the friend's own browser.
- **Consequences:** no `dynamo` change, no IAM-capture corpus change, no policy-template change, no
  terraform. The deploy is lambda code plus web assets only.
- `steam-client`: `get_wishlist(&SteamId64) -> Result<Vec<WishItem>, SteamError>`, **keyless** (no
  `key` query param). It reuses the existing `net()` URL-stripping and status mapping. A response
  without `items` returns `Ok(vec![])`. A malformed body returns `Parse`.

## 5. Client

- `steamWishlistForLink(token, steamid) -> Promise<WishItem[]>`. It throws `FetchFailed` on
  404/409/!ok, the same as `steamOwnedForLink`.
- **Fetched whenever a Steam identity is present on the page**, whether from the OpenID return or
  restored from localStorage. `SteamIdentity`'s stored shape is **unchanged**, so friends who connected
  before this ships see stars too, with no migration.
- **A wishlist failure never blocks owned** and never sets `steamError`. It degrades to no stars.
- **The float lands before the cards do (amended at implementation, from Lilith's layout-jump review).**
  On an active open shelf, the first render waits **at most 400 ms** for the wishlist. If it lands in
  time, starred games float, and the order is **frozen for the visit** from that render onward. If it
  lands later, it **adds stars and moves nothing**, because a card never slides away under a friend's
  cursor. Curated links never wait. A reload is a new visit and reshuffles, as the shelf shuffle
  already does (Ben, 2026-07-09).
- **Disconnect** clears the wishlist state along with the identity.

## 6. Non-goals

- **Nothing on the giver side.** No admin view of a friend's wishlist, and no curation assist. (A
  giver-side curation plan, `docs/superpowers/plans/2026-08-17-surfacing-engine-phase-1.md`, is
  marked RETRACTED because its premise is false. This spec does not reopen it.)
- No wishlist `priority` ordering in v1. Plenty of people do set it: measured across the 17 public
  wishlists, **879 of 1,635 items have a non-zero priority**. But the float keeps the shelf's per-visit
  shuffle inside the starred group on purpose, so the page keeps its rummage feel. Ordering starred
  games by priority is a clean follow-up if Ben wants it. (An earlier draft of this line said
  priority was almost always 0. That came from reading two items.)
- No notifications, no lantern/whisper integration, and no stored link between a friend and a Steam
  id.
- No new external request from the browser. Steam is only ever called server-side, so CSP is
  unchanged.

## 7. Acceptance

- A red-first test for each: the proxy preamble order (unknown token beats bad id64; dead link beats
  bad id64), the intersection (a wishlisted appid not on the shelf is **absent** from the response),
  the keyless request (no `key` param reaches Steam), and owned-wins precedence.
- Web: the float is a stable partition that keeps the shuffle rank inside each group; curated order is
  byte-identical with and without a wishlist; the count line is absent at 0; restore-without-field
  fetches the wishlist.
- **No `dynamo`/`terraform` diff at all.** The PR's file list proves it.
- **Deployed and verified live:** a real public-wishlist id64 against a real prod link returns the
  intersection, and a table scan afterwards finds **no** item keyed to that id.

## 8. Open questions (for the family)

1. **Float or mark-only on open shelves?** I say float. The whole point is cutting through the number
   of choices, and the shuffle stays intact within groups. Would the shelf feel less like
   rummaging?
2. **The count line:** delight or noise? It is the one element with no per-card anchor.
3. ~~Cache the full list at rest?~~ **Resolved: nothing at rest (§4)**, from OMBB's objection.
   Settled by OMBB + Lilith: **float** (§2.2); the count line **only at ≥1, never "0 of these"**,
   tested both ways, and shown on **curated links too** (Lilith: there the stars stay scattered in
   Ben's order, so the line is what tells a friend to go looking) (§2.4). Lilith's
   empty-must-overwrite arm is moot for the same reason: no server-side entry exists to go stale.
