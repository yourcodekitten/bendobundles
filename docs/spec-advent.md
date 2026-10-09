# the advent window 🕯️ — spec

*2026-10-09, kitten. Status: v1 draft for family review. Where narrative and **decisions** disagree,
the decisions win.*

## why this exists

PRODUCT.md: *"the unwrap is the product."* Today a friend unwraps once. A curated link hands over
its whole set on the first visit, and the next time the friend thinks about bendobundles is the next
time ben sends a link.

December is when ben gives. The advent window turns one curated link into **a door a day**: ben picks
up to 25 games in the order he wants them found, sets a first day, and sends one link. Each morning
(America/New_York midnight) the next door opens. The friend comes back tomorrow, and the day after,
and each visit is a small unwrap with ben's tag on it. The point is not the games. The point is
twenty-four mornings of *ben thought about this*.

## what the friend sees

- `/l/{token}` renders a **calendar of N doors** (numbered 1…N, the date under each) instead of the
  plain grid.
- **Open doors** show their game card: cover art, the gift tag, the same claim flow as a curated link
  today. Today's door is lit (one warm accent, no glow-everything).
- **Closed doors** show only their number and the date they open: *"dec 7"*. No art, no title, no
  hint. **The server never sends anything about a closed door except its number and its open
  instant.**
- A line above the calendar: *"door 8 opens tomorrow ♡"* (or *"in 3 days"* before the first door).
  After the last door: *"every door is open."*
- The first time a viewer sees a door open, it plays a short open animation (`prefers-reduced-motion`
  → plain fade). "Seen" lives in `localStorage` per token, a convenience only: if storage is missing,
  every open door renders already-open, with no animation.

## what ben sees (admin)

- On the links form, with games picked: a toggle **"advent window 🕯️"** plus a **first door** date
  (`<input type="date">`, defaulting to the next dec 1).
- A preview line under it: *"24 doors · dec 1 → dec 24 · door 24 opens dec 24 at midnight eastern"*.
- The links list shows *"🕯️ 7/24 open"* on advent links.

## decisions

- **D1 — storage: one new field, `advent_start: Option<Date>`** (a calendar date, not an instant), on
  `domain::Link`. It is written as a top-level attribute (string `YYYY-MM-DD`), read back over the
  body like the other enforcer fields, and stripped from `body`, following the house doctrine
  (`dynamo/src/lib.rs` "body for immutable identity, top-level attrs for enforcement").
  It is **create-time-only**, like `curated_game_ids`: no edit path, so the door schedule cannot move
  under a friend.
- **D2 — door k (0-based) opens at `advent_start + k days`, 00:00 America/New_York.** Local midnight
  is never ambiguous, because US transitions happen at 02:00 local. The UTC instant is found by
  evaluating the offset at `date 05:00Z`. Measured edge cases are pinned as tests: the first Sunday
  of November (midnight is still EDT) and the second Sunday of March (midnight is still EST).
- **D3 — the open set is always a PREFIX of `curated_game_ids`.** That makes ONE gate possible:
  `Link::visible_curated_ids(now) -> Option<&[String]>`.
  - Not advent: returns the whole list, unchanged.
  - Advent: returns `&ids[..opened]`.
  - **Every public reader that today reads `curated_game_ids` reads this instead:**
    - `handle_get_link` (cards)
    - `handle_post_claim` (membership gate)
    - `live_on_link`, which also covers `handle_game_detail`
    - `link_live_app_ids` (wishlist proxy)

  The admin side (scrapbook, admin list) and fulfillment's `active_promises` keep reading the full
  list on purpose: every door is a promise, including the closed ones.
- **D4 — a claim on a closed door gets exactly the existing response**: 409 *"that one isn't part
  of this gift"*. Same status, same bytes. A distinct message would be an oracle: guess a game id and
  learn whether it is behind a door.
- **D5 — wire shape.** `LinkView` gains `advent?: { doors, opened, next_opens_at?, doors_at[] }`,
  where `doors_at` holds the RFC3339 open instant of every door (instants only, no ids). `games`
  carries the opened prefix in door order, with ghosts as today. Open-shelf, curated and sealed links
  keep their exact wire shape (absent key, pinned like `open_shelf_wire_shape_is_unchanged`).
- **D6 — validation (admin-api, 422):**
  - `advent_start` requires `game_ids`.
  - 2 ≤ doors ≤ `ADVENT_DOORS_MAX = 25`.
  - `advent_start` may not be in the past (eastern date) and may be at most `UNLOCK_MAX_DAYS` ahead.
  - `unlock_at` and `advent_start` are **mutually exclusive**: a sealed advent link means two
    clocks, and ben only needs one.
  - expiry, if set, must fall after the **last** door opens.
  - `claims_allowed ≤ doors`, as for curated today.
- **D7 — the eastern clock moves into `domain`.** `fulfillment/src/lantern.rs` has a private,
  DST-pinned `eastern_offset`. Hoist it to `domain::eastern` and have the lantern call it, so both
  surfaces share one rule and one set of DST tests. *(This is open question Q1.)*
- **D8 — unfurl.** An advent link's preview says *"N little doors, one a day. chosen for you."* It
  gives the count only, never titles. Count is already public on curated links.
- **D9 — before door 1** the link is not sealed: it renders the calendar with every door closed and
  the countdown line. Claim attempts get D4's 409.

## out of scope

- Editing the schedule after creation.
- Per-door claim pacing ("one claim per day"). `claims_allowed` stays the only budget.
- Notifications ("your door is open"). The friend has no account and nothing to notify.
- Any change to open-shelf, curated or sealed behaviour.

## open questions (family)

- **Q1** (OMBB): hoisting `eastern_offset` into `domain` changes the fulfillment binary, so the deploy
  is 0/3/0, not 0/2/0. Hoist (one rule, one test set), or duplicate it in `domain` and leave the
  lantern alone?
- **Q2** (Lilith): D4 makes closed-door and not-in-gift claims indistinguishable on purpose. Any
  reason the friend UI would want to tell them apart? (The UI never offers a closed door, so only a
  hand-crafted request can hit it.)
- **Q3** (both): D3's prefix gate. Is there a public reader of `curated_game_ids` that the census
  missed? Census command: `git grep -n curated_game_ids -- crates ':!*/tests/*'`, read in full.
  The public readers are all in `public-api/src/lib.rs` plus `unfurl.rs`.

## how we'll know it worked

- A friend opening an advent link on dec 3 sees doors 1–3 open and 4–N as dates, and the network
  response contains **no** id, title, appid or art for doors 4–N (asserted in a test that greps the
  serialized body for every closed door's id).
- A hand-crafted claim on door 4's id returns byte-identical bytes to a not-in-gift claim.
- The detail and wishlist endpoints for door 4's id behave as for a game not on the link.
