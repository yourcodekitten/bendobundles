# the lantern 🏮 — spec

*2026-09-16, kitten. Status: v1 — DRAFT for family crossfire (OMBB + Lilith). Where narrative and
**decisions** disagree, the decisions win; decisions are appended as the crossfire lands.*

## why this exists

The attic already speaks to ben in two registers:

- **the whisper 💌** (weekly, Saturday) surfaces one *forgotten treasure* — unclaimed inventory.
- **the bell 🔔** rings *at events* — an unwrap, a thank-you — the moment they happen.

Nothing speaks about the **non-event**: the intention ben formed and the world never answered.
A link he cut for a friend that nobody walked through. A claim that started and never finished
(#234's specimen: **Pending since 2026-07-06 — 70 days measured on 09-14, 72 today**, paging a
webhook nobody reads, daily). A gift he wrapped for a date that came and went unopened. A door
about to close with claims still inside. Every one of these is *"an unclaimed game is a gift
nobody got to open"* (PRODUCT.md) wearing a different coat — and every one is invisible unless
ben opens the admin, which is the exact forgetting the product exists to fight.

The scrapbook composed these for the giver on **pull** (doors left open, chosen-and-waiting,
`stale_pending_count`). The lantern is the same truths on **push**, on the register ben reads —
once a week, on Sunday evening, when the week's giving is done and the attic walks its rooms
with a lantern and tells him what is still waiting.

## what it is

**One Discord message, Sunday evening, on the whisper register (ben's channel), only when there is
something to say.** Four rooms, each a section; empty rooms are omitted; all empty ⇒ no message
(and a logged no-send cause, see §silence). In the attic voice, ≤ one screen.

```
🏮 the lantern · week of sep 14

🚪 doors nobody has walked through
   · the door for sam — open 2 weeks, nobody's come through yet  ↗ links
   · the door for the d&d table — open 2 months, 5 of 5 still inside  ↗ links

🕯️ stuck in the chimney
   · soulcalibur vi — a claim started jul 6 and never finished (76 days)  ↗ ops

🎁 wrapped, and past its day
   · the gift for mira — openable since dec 25, still wrapped a week later  ↗ links

⏳ doors closing soon
   · the door for jo closes thursday with 2 claims left  ↗ links
```

Every line deep-links to the **admin** page that acts on it (`/admin/links`, `/admin/ops`),
never to the friend-facing invite. **No bearer capability rides the message** — the link token IS
the claim capability, and a Discord channel is not a vault (the whisper's deep-link rule, kept).

## the four rooms — predicates (all pure, all measured against the schema)

Let `now` be the tick time (UTC), `age(x) = now − x`. Ages in whole days.

| room | population | mention rule |
|---|---|---|
| 🚪 **doors** | links where `can_claim(now).is_ok()` (live: unrevoked, unexpired, unsealed, claims remain — domain's own gate, the scrapbook's `link_is_open_door`) **and `claims_used == 0`** | **birthdays, stateless:** mentioned in the tick whose window `[age, age+7d)` contains **14d** or **60d** since `created_at`. Once at two weeks ("nobody's come through yet"), once at two months ("still inside — shall it stay open?"). Never otherwise. |
| 🕯️ **chimney** | claims with `state == Pending` and `age(created_at) ≥ RECONCILE_STUCK_ALERT_AGE` (24h — the sweep's own bar, `fulfillment/src/lib.rs:4201`) | **every tick while stuck.** This is ops truth on a warm register: the 2026-07-29 family ruling — *a once-ever alert that scrolls away IS the silent-loop bug* — applies. The daily ops ping is untouched (#162 stays its own decision). |
| 🎁 **wrapped** | links with `unlock_at` set, `unlock_at ≤ now`, `claims_used == 0`, otherwise live | **once:** the tick whose window contains **7d after `unlock_at`**. |
| ⏳ **closing** | links live with claims remaining and `expires_at` within `(now, now+7d]` | **once, by construction:** exactly one weekly window contains a given `expires_at`. |

**Why birthdays and not a marker table.** A per-link "mentioned" record is state that can drift,
needs a schema, and makes the tick a writer of N items. A birthday is a pure function of
`(created_at, now)` and the weekly cadence: each door gets its 14d line in exactly one tick and
its 60d line in exactly one tick, with **zero writes**. A missed tick loses that mention — the
never-ran alarm (§mechanism) owns "the tick didn't run", the same split the whisper uses.
⚠️ Coupled to the weekly cadence exactly like the whisper's slot key: a sub-weekly schedule must
change the window width in the same commit. The window is `[age, age+7d)` **half-open** so a door
born exactly 14d before a tick is mentioned by that tick and not the next.

**Measured launch state (prod, kitten-debug read-only, 2026-09-16T07:2x):** 18 links; 11
unrevoked with `claims_used = 0`, created 07-07 → 08-05 (ages 42–71d on 09-16); `expires_at` and
`unlock_at` absent on all 18; `curated_game_ids` absent on all 18 (still the 08-28 census).
⇒ **The first lantern (Sun 09-20 17:00 ET) carries: 0 doors, 1 chimney line (the #234 claim, if
still Pending), 0 wrapped, 0 closing.** The 07-28 door hits its 60d window on 09-27; the 08-05
door on 10-04. The July doors are past both birthdays and will never be mentioned — **the lantern
lights forward, like the bell; no retroactive sweep at deploy** (decision, not omission).

## the message

- `content` = header line + rooms as plain text (the whisper v1 shape, not v2 embeds: this is a
  list, not a card; embeds would fight the one-screen rule). `allowed_mentions: {"parse": []}`.
- **Per-room cap 5 lines + "· and N more ↗"**; total content capped at 2000 (Discord's hard
  limit) with the same `cap()` helper the bell uses. Ordering inside a room: oldest first.
- Recipient naming reuses the scrapbook's rule: `Friend.name` via `friend_id`, else the link
  `label`. (Measured: no link carries `friend_id` yet; labels it is.)
- Game titles come from `Game.title` via `game_id`; a missing game renders as the id, never
  drops the line (the scrapbook's "orphans are COUNTED, never skipped").
- Dates in the message are **America/New_York** (ben reads local; the tick is tz-aware).

## mechanism — reuses, deliberately, the whisper's skeleton

```
aws_scheduler_schedule "lantern"  cron(0 17 ? * SUN *)  America/New_York
  └─ fulfillment lambda, input {"op":"lantern"} → FulfillRequest::Lantern
       ├─ GATE  whisper register resolves Webhook?  else loud no-op, ZERO writes (dark-deploy rule)
       ├─ READ  list_links · list_pending_claims · games for the mentioned ids · friends
       ├─ COMPOSE  lantern::compose(links, claims, games, friends, now) -> Option<Lantern>
       │      None ⇒ log outcome=lantern_quiet (nothing to say) — no write, no send
       ├─ RECORD  LANTERN#<iso-week> conditional put (idempotence: one lantern per week)
       ├─ SEND    whisper_send_body(http, url, body)  — the ONE webhook POST function
       └─ MARK    delivered
```

- **Register:** the WHISPER webhook (`WHISPER_WEBHOOK_PARAM`), same credential, same room —
  it is ben's channel and this is for ben. **Off-switch:** `LANTERN_DISABLED=1`, read ONLY by
  the lantern (the bell/whisper register-decoupling rule: one room, one rotation event, separate
  mutes). Never `NOTIFY_DISABLED`.
- **Idempotence** = the whisper's `WHISPER#<slot>` pattern with its own key prefix
  `LANTERN#<iso-week>`; never touches `WHISPER#` state. Slot computed from the tick's UTC date
  → **17:00 ET is 21:00Z (EDT) / 22:00Z (EST): 3h / 2h inside the same UTC Sunday**, so the
  ISO week is the tick's own in both seasons. Documented on the tf variable exactly as the
  whisper's is — *do not move the tick past 19:00 ET without re-deriving.*
- **Preview op** `{"op":"lantern_preview"}` (manual invoke only, like `whisper_preview`):
  composes against live data, POSTs with a `(preview)` header, **zero writes**. This is the
  deploy-verification instrument — and, since the message IS the reveal, step 12 and step 14 of
  the pounce happen minutes apart by design.
- **The run that never happened:** clone `whisper_never_ran` for the lantern's schedule group.
  ⚠️ The whisper needed a Sunday heartbeat tick to keep the metric present at ≤6-day gaps under
  AWS's 7-bucket cap. The lantern gets the same shape: **a Wednesday 17:00 heartbeat tick that
  loses the conditional put by design** — *not* a second lantern. (Q③ below asks whether a
  heartbeat that composes-and-discards is acceptable, or whether the heartbeat should be a
  distinct `{"op":"lantern_heartbeat"}` that only touches the metric.)
- **Terraform:** `lantern_enabled` (default false → dark deploy), `lantern_schedule_expression`,
  schedule group + schedule + scheduler role (copies of the whisper trio), alarm clone,
  `LANTERN_DISABLED` env plumbed. No new IAM beyond invoking the existing lambda and the
  existing SSM read (the register already exists).

### silence is a state, not an absence

Five ways a Sunday goes by with no lantern, each with its own logged `outcome=`:
`lantern_dark` (register UNSET/disabled — loud, zero writes) · `lantern_quiet` (all rooms empty —
the healthy case) · `lantern_slot_taken` (heartbeat/second tick lost the put — by design) ·
`lantern_read_failed` (store error — next week retries; the pending sweep's rule) ·
`lantern_send_failed` (POST failed after RECORD — the row exists undelivered; next week's tick
is a fresh slot, and the undelivered row is the audit trail, same as the whisper).
**A quiet lantern sends nothing to ben** — Q① asks whether that's right.

## code shape

- `crates/domain`: **relocate** the scrapbook's `link_waits` / `link_is_open_door` predicates
  onto `Link` (`is_open_door(now)`, `waits(now)`) and point `scrapbook.rs` at them — one copy,
  the repo's rule; the lantern is the second caller, which is when a copy becomes a mechanism.
  Existing scrapbook tests pin the behaviour across the move.
- `crates/fulfillment/src/lantern.rs`: pure `compose(...)` + `render(...)`, unit-tested against
  fixtures: each room's boundary (13d/14d/20d/21d · 59d/60d/66d/67d · unlock+6d/+7d/+13d/+14d ·
  expires now+7d inclusive / +7d+1s exclusive), the 24h chimney bar, per-room cap + "and N more",
  the 2000 cap, orphan game id rendered not dropped, all-empty ⇒ `None`.
- `FulfillRequest::Lantern` / `LanternPreview` + `handle_lantern` — the whisper handler's
  shape with the lantern's composer; `record_lantern` / `mark_lantern_delivered` in `dynamo`
  (copies of the whisper trio with the new prefix; `list_lanterns` for the preview's "newest").
- Tests pin: LANTERN_DISABLED darkens only the lantern (bell + whisper unaffected, and vice
  versa); dark register ⇒ zero store writes; quiet ⇒ zero writes; slot-taken ⇒ no send.

## non-goals (decided, not omitted)

- **No action buttons / no closing doors from Discord.** Read-and-go-look; the admin acts.
- **No invite URLs in the message.** See "no bearer capability".
- **No retroactive mentions** for doors already past their birthdays at deploy.
- **Not a replacement for the ops register.** The daily stuck-claim ping to ops stays exactly as
  it is (#162 is a separate decision); the lantern is the READ surface beside it.
- **No embeds/art.** A list is a list. The whisper is where the art lives.

## open questions for the family

- **Q① quiet weeks.** All-empty ⇒ no message. The alternative is a monthly "all quiet in the
  attic ♡" so silence is distinguishable from breakage *from ben's seat*. My position: the
  never-ran alarm distinguishes them from *ours*, and ben should hear from the lantern only when
  it has something — a heartbeat to a human is the nag the whisper spec forbade. Push back.
- **Q② the chimney on a warm register.** Repeating "soulcalibur vi, 76 days" every Sunday until
  it's resolved is honest and slightly grim. Options: (a) every week (proposed); (b) weeks 1, 2,
  4, 8… (backoff); (c) once + a `still stuck, week N` counter. I hold (a) because the 07-29 ruling
  was made about exactly this claim class, but that ruling was for the OPS register.
- **Q③ heartbeat shape.** Compose-and-discard on the Wednesday tick (simple, one code path, but
  runs the reads for nothing) vs a distinct metric-only op (cheaper, second code path). Lean:
  compose-and-discard, because the whisper's heartbeat already established the pattern and a
  second op is a second thing to keep honest.
- **Q④ door birthdays: 14d and 60d.** Are two mentions right, and are these the days? The
  numbers are mine; the shape (finite, stateless, never a weekly nag) is the claim.
- **Q⑤ open-shelf doors.** 10 of the 11 zero-claim links are open shelves (`claims_allowed` 5–15,
  no curation, no friend) — standing invitations to a group, not a door cut for one person. Should
  the doors room require `claims_allowed == 1 || friend_id.is_some() || curated`? My lean: **yes**
  — a shelf with 15 slots that nobody has used in two months is still a door nobody walked
  through, but the *voice* ("the door for sam") is wrong for it. Proposal: mention shelves too,
  with shelf voice ("the shelf for the d&d table — 15 slots, nobody's taken one in 2 months").
