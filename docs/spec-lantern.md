# the lantern 🏮 — spec

*2026-09-16, kitten. Status: v4 — OMBB round 1 (B1, B2a, B2b, major, Q①–Q⑤) + Lilith round 1
(closing looks forward, backlog line, chimney names its action, bell markdown finding) + OMBB
round 2 (closing on k+1) + round 3 (F1: quiet writes a row) integrated; **go-to-plan given at
`3d56bbb`; OMBB plan SIGN-OFF at plan `f204d13` / spec `6d0d34f` (07:48), scoped to D1+D2**. Where narrative and **decisions** disagree, the decisions win.*

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
🏮 the lantern · week of sep 13

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
| 🚪 **doors** | links where `can_claim(now).is_ok()` (live: unrevoked, unexpired, unsealed, claims remain — domain's own gate, the scrapbook's `link_is_open_door`) **and `claims_used == 0`** | **birthdays, stateless, bucketed by SLOT (decision B2):** mentioned in the tick whose bucket contains the instant `created_at + 14d`, and again the one containing `created_at + 60d`. Once at two weeks ("nobody's come through yet"), once at two months ("still inside — shall it stay open?"). Never otherwise. **Voice keys on shape (Q⑤):** `claims_allowed > 1 && friend_id.is_none() && curated_game_ids.is_none()` ⇒ *shelf* voice ("the shelf for the d&d table — 15 slots, nobody's taken one in 2 months"); else *door* voice ("the door for sam"). Fixtures on both sides. |
| 🕯️ **chimney** | claims with `state == Pending` and `age(created_at) ≥ RECONCILE_STUCK_ALERT_AGE` (24h — the sweep's own bar, `fulfillment/src/lib.rs:4201`) | **every tick while stuck, with a stateless `week N` counter** (`N = age.whole_days() / 7`, Q② = (a)+(c)). Ops truth on a warm register: the 2026-07-29 family ruling — *a once-ever alert that scrolls away IS the silent-loop bug* — applies; backoff rebuilds the silent loop. The daily ops ping is untouched (#162 stays its own decision). |
| 🎁 **wrapped** | links with `unlock_at` set, `unlock_at ≤ now`, `claims_used == 0`, otherwise live | **once:** the tick whose bucket contains `unlock_at + 7d`. |
| ⏳ **closing** | links live with claims remaining and `expires_at` set | **once, LOOKING FORWARD (Lilith):** mentioned at tick *k* iff `expires_at ∈ BUCKET(k+1)` — the coming week, `[BOUNDARY(k), BOUNDARY(k+1))`. Doors and wrapped look BACK (their instant is a past birthday, bucket *k*); closing is the one room whose instant is in the future. An expiry earlier in the tick's own bucket is already closed and must NOT appear (fixture). An expiry in the hours right after Sunday's tick is in BUCKET(k+1) and is named by tick *k* ("closes tonight"), still exactly once. |

### the bucket function — decision B2 (OMBB, 2026-09-16, blockers B2a + B2b)

v1 wrote the window as `[age, age+7d) ∋ 14d`, which fires at **8 days** old (inverted — the
fixtures 13/14/20/21 were the intent, the formula was not), and ANY `now`-anchored window leaks:
consecutive 17:00 ET ticks are **169h** apart across fall-back and **167h** across spring-forward,
so a birthday in the fall-back hour is mentioned never and one in the spring-forward hour twice;
a late scheduler retry shifts every window the same way. **Fix: partition time by the SCHEDULE,
not by `now`.**

```
BOUNDARY(k)  = the k-th Sunday at 21:00Z              (fixed in UTC, never in local time)
BUCKET(k)    = [BOUNDARY(k-1), BOUNDARY(k))           half-open, exactly one bucket per instant
SLOT(k)      = the date of BOUNDARY(k)'s Sunday, e.g. "2026-09-20"   → LANTERN#2026-09-20
tick_slot(now) = SLOT of the latest BOUNDARY ≤ now
mention(instant, k) ⇔ instant ∈ BUCKET(k)      — doors (created_at+14d, +60d), wrapped (unlock_at+7d)
mention(instant, k) ⇔ instant ∈ BUCKET(k+1)    — closing (expires_at): the coming week
```

- The Sunday 17:05 ET tick is **21:05Z under EDT and 22:05Z under EST** — always **>** its own
  boundary, so every instant in the bucket is in the past at the tick. Under EST the span
  21:00Z–22:05Z belongs to the *next* bucket: for **doors and wrapped** that means mentioned a
  week later, never dropped, never doubled. (**Closing is the exception, harmlessly:** an expiry
  in that span is in BUCKET(k+1) but already past at the 22:05Z tick, so liveness drops it —
  mentioned never, and the door is already closed. "Never dropped" is a doors/wrapped property.)
  Jitter and retries evaluate the same `tick_slot`, so they see the same bucket.
- ⚠️ Why NOT "ISO week of the birthday instant == tick's ISO week": the Sunday tick is the LAST
  day of its ISO week, so the 3h tail Sun 21:00Z–24:00Z is in the tick's week but after the
  tick — a birthday there would be mentioned never. The 21:00Z boundary is chosen so the tick
  always closes its own bucket.
- **Slot key is the Sunday DATE, not the ISO week** — the ISO-week key is what made the
  heartbeat win (B1 below); a Sunday-date key makes "which Sunday does this tick belong to" a
  pure function every tick agrees on.
- **Fixtures pinned:** each room at boundary −1s / +0s / +7d−1s / +7d; the fall-back week
  (Sun 2026-11-01 tick, 169h) and spring-forward week (Sun 2027-03-14 tick, 167h) each show every
  instant in exactly one bucket; a Wednesday `now` maps to the previous Sunday's slot; **closing:
  an expiry on the Thursday BEFORE the tick does not appear, one on the Thursday AFTER does, one
  at BOUNDARY(k)+1s appears at tick k and not k+1** (Lilith's in-week-expiry fixture beside
  OMBB's DST one).

**Launch state under B2 (re-derived):** 07-28 door → `+60d` = 09-26T18:43Z ∈ BUCKET ending
09-27 ⇒ mentioned **09-27**; 08-05 door → `+60d` = 10-04T14:09Z ⇒ mentioned **10-04**. First
lantern (09-20) still carries **0 doors · 1 chimney · 0 wrapped · 0 closing**, now under the
formula and the fixtures both.

**Why birthdays and not a marker table.** A per-link "mentioned" record is state that can drift,
needs a schema, and makes the tick a writer of N items. A birthday is a pure function of
`(created_at, slot)` and the weekly cadence: each door gets its 14d line in exactly one bucket
and its 60d line in exactly one bucket, with **zero writes**. A bucket whose lantern never
DELIVERS loses those mentions — which is why delivery gets a retry inside the slot (§heartbeat)
and a logged outcome, not just an alarm on a room nobody reads. A tick that never RUNS is the
never-ran alarm's (§mechanism), the same split the whisper uses.
⚠️ Coupled to the weekly cadence exactly like the whisper's slot key: a sub-weekly schedule must
change the bucket function in the same commit.

**Measured launch state (prod, kitten-debug read-only, 2026-09-16T07:0x–07:2x):** 18 links; 11
unrevoked with `claims_used = 0`, created 07-07 → 08-05 (ages 42–71d on 09-16); `expires_at` and
`unlock_at` absent on all 18; `curated_game_ids` absent on all 18 (still the 08-28 census);
**exactly 1 Pending claim** — queried through the sweep's own `pending-claims` GSI (`gsi2pk =
PENDINGCLAIM`), the #234 specimen, 72 days. **The first lantern (Sun 09-20 17:00 ET) carries:
0 doors, 1 chimney line, 0 wrapped, 0 closing.** The July doors are past both birthdays and will
never be mentioned — **the lantern lights forward, like the bell; no retroactive sweep at deploy**
(decision, not omission). **#234's disposition is NOT a launch gate** (OMBB asked; measured: no
admin surface can compensate a self-claim today, and the claim is ben's own — its disposition is
his call, which the lantern's first line is precisely the mechanism for raising; it goes in the
reveal with the two options, compensate or leave). One honest line, until he acts, is the
feature working — not the feature nagging.

### the backlog line — decision (Lilith): "lights forward" does not carry over from the bell

The bell is about events; a missed event is gone. The lantern is about **state that is still
standing** — 9 of the 11 open doors are past both birthdays at launch and would never be
mentioned, which is the exact forgetting the first paragraph describes, left in place. **Fix, with
no new state:** while **no lantern has ever been DELIVERED** (`list_lanterns` has no row with
`delivered == true`), the doors room carries one extra summary line — *"and N doors older than
two months nobody has walked through ↗ links"* — counting every open zero-claim link whose 60d
birthday is already past. One line, once. **Keyed on a DELIVERED row, not any row**, so a failed
first send (row present, undelivered) does not use it up — the heartbeat's resend recomposes it.
**`lantern_preview` writes nothing, so it cannot consume the line — pinned by a test** (OMBB):
preview → zero writes → a following real tick still carries the backlog line.

### the chimney line names the action that clears it — decision (Lilith, OMBB)

A number that only grows is the repeating alert with a nicer font — the exact message ben has
told us he learns to ignore. Each chimney line says what the state IS and what CLEARS it:
*"soulcalibur vi — a claim started jul 6 never finished (week 10). it clears when the claim is
compensated (slot returned, game re-listed) or fulfilled — no admin button for that yet, see
#234 ↗ ops"*. Honest about the missing button: `compensate_self_claim` exists in fulfillment and
nothing in the admin can invoke it (measured 2026-09-16). **Follow-up issue filed with the PR:**
*admin ops — compensate a stuck Pending claim* (the button the line names). Until it exists the
line names the action AND the gap; the reveal puts the #234 disposition to ben as two options.

## the message

- `content` = header line + rooms as plain text (the whisper v1 shape, not v2 embeds: this is a
  list, not a card; embeds would fight the one-screen rule). `allowed_mentions: {"parse": []}`.
- **Per-room cap 5 lines + "· and N more ↗"**; total content capped at 2000 (Discord's hard
  limit) with the same `cap()` helper the bell uses. Ordering inside a room: oldest first.
- Recipient naming reuses the scrapbook's rule: `Friend.name` via `friend_id`, else the link
  `label`. (Measured: no link carries `friend_id` yet; labels it is.)
- Game titles come from `Game.title` via `game_id`; a missing game renders as the id, never
  drops the line (the scrapbook's "orphans are COUNTED, never skipped").
- **Every interpolated string is Discord-Markdown-escaped** — decision from Lilith's bell finding
  (OMBB confirmed at `65b9c78`): `bell.rs:205` puts the friend-written `thank_note` raw into
  `content`, sanitised for control/bidi and capped, but Markdown is not escaped and Discord
  renders masked links in webhook content — `[open your gift](https://…)` becomes a clickable
  link with made-up text. `allowed_mentions` stops pings, not links. The lantern interpolates
  **labels (admin-written), friend names (admin-written) and game TITLES (Humble/Steam-sourced,
  third-party text)** — a title with `*`, `_` or brackets renders wrong at best. ⇒ **one helper,
  `discord::escape_md(&str)`, escaping `\ * _ ~ \` | > [ ] ( )`, in the shared send module; the
  lantern uses it on every field and the bell's `thanks_card` adopts it in the same PR** (the
  repo's one-copy rule; a code span was rejected — a backtick in the note breaks out of it).
  Tests: a masked link + a stray backtick render inert; the escape is idempotent on plain text.
- Dates in the message are **America/New_York** (ben reads local; the tick is tz-aware).

## mechanism — reuses, deliberately, the whisper's skeleton

```
aws_scheduler_schedule "lantern"  cron(5 17 ? * SUN *)  America/New_York   (heartbeat: WED, same time)
  └─ fulfillment lambda, input {"op":"lantern"} → FulfillRequest::Lantern
       ├─ GATE  whisper register resolves Webhook?  else loud no-op, ZERO writes (dark-deploy rule)
       ├─ READ  list_links · list_pending_claims · games for the mentioned ids · friends
       ├─ COMPOSE  lantern::compose(links, claims, games, friends, slot, now) -> Option<Lantern>
       │      None ⇒ log outcome=lantern_quiet (nothing to say) — RECORD a `quiet` slot row
       │             (one write, zero sends; decision F1) and exit
       ├─ RECORD  LANTERN#<iso-week> conditional put (idempotence: one lantern per week)
       ├─ SEND    whisper_send_body(http, url, body)  — the ONE webhook POST function
       └─ MARK    delivered
```

- **Register:** the WHISPER webhook (`WHISPER_WEBHOOK_PARAM`), same credential, same room —
  it is ben's channel and this is for ben. **Off-switch:** `LANTERN_DISABLED=1`, read ONLY by
  the lantern (the bell/whisper register-decoupling rule: one room, one rotation event, separate
  mutes). Never `NOTIFY_DISABLED`. **Made real (plan gate, OMBB): the lantern resolves ITS OWN
  `Notify` from the whisper's `SecretRead` with its own flag** — a `bool` beside `whisper_notify`
  routed through `resolve_whisper_url` would inherit `WHISPER_DISABLED` (the bell has exactly that
  coupling today; follow-up issue).
- **Idempotence** = the whisper's `WHISPER#<slot>` pattern with its own key prefix
  `LANTERN#<sunday-date>` (decision B1/B2: the key is `tick_slot(now)`, NOT an ISO week); never
  touches `WHISPER#` state. **The tick is 17:05 ET = 21:05Z (EDT) / 22:05Z (EST): five minutes
  AFTER the 21:00Z boundary (a tick ON the boundary has zero margin against clock skew — a
  20:59:59.9 reading maps to LAST week's slot and exits `slot_taken`, which reads healthy), and
  2h55 / 1h55 of margin to the UTC-midnight cliff, stated on the tf variable** (the whisper's
  10h/9h figure does not transfer). *Do not move the tick without re-deriving both edges.*
- **Preview op** `{"op":"lantern_preview"}` (manual invoke only, like `whisper_preview`):
  composes against live data, POSTs with a `(preview)` header, **zero writes**. This is the
  deploy-verification instrument — and, since the message IS the reveal, step 12 and step 14 of
  the pounce happen minutes apart by design.
- **The run that never happened:** clone `whisper_never_ran` on the lantern's OWN schedule group
  (AWS/Scheduler metrics carry exactly one dimension, the group — a shared group masks silence).
  The whisper needed a second weekly tick to keep the metric present at ≤6-day gaps under AWS's
  7-bucket cap; the lantern needs one too.
- **Decision B1 (OMBB, blocker): the heartbeat CANNOT be an ISO-week loser.** The whisper's
  Sunday heartbeat loses only because Saturday precedes Sunday inside one ISO week. A lantern
  that ticks on SUNDAY — the last ISO day — has no later day in its week: a Wednesday tick keyed
  by ISO week is in the NEXT week, wins its slot, and sends a lantern on a Wednesday while the
  real Sunday exits `slot_taken`. Two changes, together:
  ① **the slot key is the Sunday DATE via `tick_slot(now)`** (§bucket function) — a Wednesday
  `now` maps to the previous Sunday's slot, so the heartbeat is a loser BY CONSTRUCTION, on
  every calendar, and
  ② **the Wednesday tick is `{"op":"lantern_heartbeat"}` — a distinct op that reads the slot
  row and does exactly one of these:** row absent AND some lantern row exists ⇒ the Sunday tick
  never RAN (schedule fault, or read-failed before RECORD) ⇒ run the full lantern for that slot
  (the retry day the whisper's heartbeat also is); **row absent AND ZERO rows ⇒ metric only +
  `outcome=lantern_heartbeat_no_history`** (OMBB, plan gate: enabling on Mon–Wed must not send a
  lantern for the Sunday before the lantern existed — *absent* there means "did not exist yet";
  residual stated: with zero history a Sunday schedule that never fires is masked by its own
  heartbeat until the first real Sunday, which the deploy checklist watches); row present +
  (`delivered` OR `quiet`) ⇒ exit, metric touched, nothing else; row present + **undelivered and
  not quiet** ⇒ **RESEND the same slot and mark delivered** — the answer to the major below —
  **or, if it now composes EMPTY, settle it as `quiet`** (never `delivered`: nothing was sent, and
  `delivered` keys the backlog line).
  🛑 **Decision F1 (OMBB, round 3): a QUIET Sunday must leave a row.** With "quiet ⇒ zero
  writes", Wednesday reads *absent* and runs the full lantern — and the chimney check uses
  `now`, so a claim that started Monday is ≥24h Pending by Wednesday and ben gets a Wednesday
  lantern. *Absent* meant both "never ran" and "ran, said nothing" and the heartbeat could not
  tell them apart. ⇒ quiet RECORDs `LANTERN#<sunday>` with `quiet = true, delivered = false`
  (one write a week, zero sends); the heartbeat treats `quiet` like `delivered`. A quiet row is
  NOT a delivered row, so it never consumes the backlog line (which cannot coincide with a quiet
  week anyway — a non-zero backlog makes the doors room non-empty). It never composes a different week's lantern: `tick_slot` pins it to Sunday's.
  (OMBB proposed a metric-only op; this is that op plus the one branch that makes a failed
  Sunday recoverable without a second alarm on a room nobody reads. If the family prefers the
  pure metric-only form, the undelivered branch is one `if` to delete and the major reopens.)
- **Major (OMBB): a failed POST silently eats a bucket's birthdays.** Stateless means the
  door/wrapped/closing lines of that slot exist in no other tick. The never-ran alarm cannot see
  it — the tick ran. Closed by: the heartbeat's undelivered-resend branch (above) gives every
  slot a second delivery attempt 3 days later — **same bucket, current liveness** (the buckets
  are fixed by the slot; `can_claim(now)`, the chimney's age and every liveness check are
  re-evaluated on Wednesday, so a door claimed on Tuesday is rightly gone); both failures log
  `outcome=lantern_send_failed` with the slot AND ping ops (whisper parity); the undelivered row
  is durable and the preview op logs the undelivered count. **Delivery is AT-LEAST-ONCE:** a
  POST that lands whose MARK then fails (timeout) leaves the row undelivered and Wednesday sends
  a duplicate — a duplicate beats lost mentions, written down here so nobody "fixes" it into
  at-most-once and reopens the major.
- **Terraform:** `lantern_enabled` (default false → dark deploy), `lantern_schedule_expression`
  + `lantern_heartbeat_schedule_expression`, schedule group + two schedules + scheduler role
  (copies of the whisper trio), alarm clones. **`LANTERN_DISABLED` is NOT terraform-plumbed** —
  parity with `BELL_DISABLED` (no `*_DISABLED` is; plan review Q①): an operator mute is a manual
  env edit on the lambda. No new IAM beyond invoking the existing lambda and the existing SSM
  read (the register already exists).

### silence is a state, not an absence

Five ways a Sunday goes by with no lantern, each with its own logged `outcome=`:
`lantern_dark` (register UNSET/disabled — loud, zero writes) · `lantern_quiet` (all rooms empty —
the healthy case; **logs the per-room counts AND the population sizes it judged** — links read,
pending read — so a predicate bug reads as "0 of 18" and not as peace; Q① decision) ·
`lantern_slot_taken` (a second tick lost the put — by design) · `lantern_read_failed` (store
error — the Wednesday heartbeat retries the slot; the pending sweep's rule) · `lantern_send_failed`
(POST failed after RECORD — the row exists undelivered; **the Wednesday heartbeat resends it**;
a second failure pings ops and the row stays as the audit trail). **`lantern_quiet` writes its
row (F1)** — the only outcome besides a send that leaves one.
**A quiet lantern sends nothing to ben** (Q① decided: the alarm proves the tick ran, the
per-room log proves compose judged the real population; a monthly "all quiet" to a human is the
nag the whisper spec forbade).
⚠️ Unlike the whisper — where an empty pool is "a predicate problem or an empty attic, never a
quiet week" and pages ops — **the lantern's empty is a healthy state and must NOT page.** The
handler must not inherit that branch from the whisper's skeleton.

## code shape

- `crates/domain`: **relocate** the scrapbook's `link_waits` / `link_is_open_door` predicates
  onto `Link` (`is_open_door(now)`, `waits(now)`) and point `scrapbook.rs` at them — one copy,
  the repo's rule; the lantern is the second caller, which is when a copy becomes a mechanism.
  Existing scrapbook tests pin the behaviour across the move.
- `crates/fulfillment/src/lantern.rs`: pure `bucket(instant) -> Slot`, `tick_slot(now) -> Slot`,
  `compose(links, claims, games, friends, slot) -> Option<Lantern>` + `render(...)`, unit-tested
  against fixtures: each room at bucket boundary −1s/+0s/+7d−1s/+7d; the fall-back (2026-11-01)
  and spring-forward (2027-03-14) weeks; Wednesday `now` → previous Sunday's slot; the 24h
  chimney bar + `week N`; shelf-vs-door voice on both sides of the predicate; per-room cap +
  "and N more"; the 2000 cap; orphan game id rendered not dropped; all-empty ⇒ `None`.
- `FulfillRequest::Lantern` / `LanternHeartbeat` / `LanternPreview` + `handle_lantern`,
  `handle_lantern_heartbeat` — the whisper handler's shape with the lantern's composer, MINUS
  the empty-pool ops page; `record_lantern` / `mark_lantern_delivered` / `list_lanterns` in
  `dynamo` (copies of the whisper trio with the new prefix and a `delivered` read for the
  heartbeat's three-way branch).
- Tests pin: LANTERN_DISABLED darkens only the lantern (bell + whisper unaffected, and vice
  versa); dark register ⇒ zero store writes; **quiet ⇒ exactly one row (`quiet = true`), zero
  sends** + per-room counts logged; slot-taken ⇒ no send; heartbeat: absent ⇒ full run ·
  delivered ⇒ no send · **quiet ⇒ no send (F1)** · undelivered ⇒ resend + mark, same slot;
  backlog line present iff no delivered row (a quiet row does not count), and a preview (zero
  writes) leaves it present; `escape_md` on masked link + backtick; the bell's thanks card
  escapes.

## non-goals (decided, not omitted)

- **No action buttons / no closing doors from Discord.** Read-and-go-look; the admin acts.
- **No invite URLs in the message.** See "no bearer capability".
- **No retroactive mentions** for doors already past their birthdays at deploy.
- **Not a replacement for the ops register.** The daily stuck-claim ping to ops stays exactly as
  it is (#162 is a separate decision); the lantern is the READ surface beside it.
- **No embeds/art.** A list is a list. The whisper is where the art lives.

## decisions (round 1 — OMBB, 2026-09-16T07:0x; Lilith's round pending)

- **B1 — heartbeat wins the ISO-week slot → BLOCKER, accepted.** Fixed by the Sunday-date slot
  key + the distinct `lantern_heartbeat` op (§mechanism). Verified: Sun 09-20 = W38, Wed 09-23 =
  W39, Sun 09-27 = W39 — under an ISO key Wednesday sends. Under `tick_slot`, Wed 09-23 → slot
  `2026-09-20` (taken) and Wed 09-30 → `2026-09-27` (taken).
- **B2a — window formula inverted (fires at 8d) → BLOCKER, accepted.** The fixtures were the
  intent; the formula was wrong. Replaced by the bucket function.
- **B2b — `now`-anchored windows leak across DST (169h/167h) and retries → BLOCKER, accepted.**
  Bucket by the schedule's fixed 21:00Z Sunday boundaries; DST-week fixtures pinned. OMBB's
  "ISO week of the instant" form was refined to the 21:00Z boundary because the Sunday tick is
  the last ISO day and its 3h tail would be unreachable.
- **Major — a failed POST eats a bucket's birthdays → accepted.** Heartbeat resends an undelivered
  slot; both outcomes logged + ops-pinged; undelivered rows durable and reported by the preview.
- **Q① quiet ⇒ silence to ben — agreed**, with per-room counts and judged population sizes logged
  on `lantern_quiet` so a predicate bug is catchable, not merely survived.
- **Q② chimney: weekly + `week N` — (a)+(c).** Backoff rebuilds the silent loop. #234's
  disposition is ben's and rides the reveal, not the launch gate (measured: no admin compensate
  surface exists; the claim is his own self-claim).
- **Q③ heartbeat: distinct op — forced by B1.** With the one undelivered-resend branch (open to
  the family: delete it and the major reopens).
- **Q④ 14d / 60d — fine.** The 60d line earns "shall it stay open?" as a real decision prompt.
- **Q⑤ shelves: INCLUDED, in shelf voice.** Excluding them drops 10 of 11 live cases and it is
  the same forgetting. Voice predicate `claims_allowed > 1 && friend_id.is_none() &&
  curated_game_ids.is_none()`, fixtures on both sides.
- **UTC cliff margin stated as the lantern's own: 3h EDT / 2h EST**, on the tf variable.

### round 2 (Lilith + OMBB, 2026-09-16T07:1x)
- **Closing looks FORWARD → BLOCKER on B2's first form, accepted by all three.** `expires_at ∈
  BUCKET(k+1)`; doors/wrapped stay on BUCKET(k). In-week-expiry fixture beside the DST fixture.
- **Backlog line, once, keyed on a DELIVERED row; preview cannot consume it — accepted.**
- **Chimney lines name the clearing action — accepted**, honestly including that the button does
  not exist yet; follow-up issue for the admin compensate action; #234's disposition rides the
  reveal.
- **Bell Markdown finding (Lilith; OMBB confirmed) — not a lantern blocker; FIXED IN THIS PR** via
  the shared `escape_md` the lantern needs anyway. Code span rejected (backtick breakout).

### round 3 (OMBB, 2026-09-16T07:13, read at `3d56bbb`) — go-to-plan, with F1 as a decision
- **F1 — quiet writes no row ⇒ the heartbeat re-runs a quiet Sunday on Wednesday → accepted.**
  Quiet records a `quiet = true` slot row; heartbeat treats it like delivered; the quiet test
  becomes "exactly one row, zero sends".
- The Sunday-date key + 21:00Z boundary refinement of B2: **taken**.
