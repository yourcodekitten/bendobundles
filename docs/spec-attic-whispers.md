# spec — the attic whispers 💌

> **PAYLOAD SUPERSEDED (2026-08-31):** the whisper's Discord payload is now the full details card
> — embeds, media galleries, preview envelope — specced in `docs/spec-whisper-details-card.md`.
> Everything below about SELECTION, the RECORD→SEND→MARK orchestration, the slot key and the five
> no-send causes remains authoritative and unchanged.

*2026-08-28 · kitten · status: family-reviewed (round 1: OMBB + Lilith, 2026-08-28 morning), ready for plan*

## the gap this closes

PRODUCT.md, the core sentence: bendobundles exists *"because ben forgets his bundles exist 95% of
the time — and because an unclaimed game is a gift nobody got to open."*

Everything the app does today is **pull**: Ben has to remember the attic exists before any gift can
happen. Nothing fights the forgetting itself. The attic sits silent between visits.

This feature makes the attic **whisper**: on a gentle schedule, one forgotten treasure surfaces in
Ben's Discord — art, the bundle it arrived in, and a one-tap path to cut a gift link for it.
Not a digest, not a dashboard, not a nag. One game, warmly, then quiet again.

## the shape

```
EventBridge Scheduler (America/New_York — Saturday morning MEANS Saturday morning across DST)
  └─ fulfillment lambda, payload {"op":"whisper"} → FulfillRequest::Whisper
       ├─ GATE: whisper webhook configured? UNSET ⇒ loud no-op, ZERO writes (see dark-deploy rule)
       ├─ SELECT one candidate (predicate below)
       ├─ RECORD a whisper-log item (conditional put — the idempotence gate)
       ├─ SEND one Discord message via the whisper webhook (art, context, deep-link)
       └─ MARK the log item delivered
```

Reuses, deliberately: the typed-envelope dispatch in `fulfillment` (the Scheduler target carries a
static `{"op":"whisper"}` input, caught by the existing `FulfillRequest` parse before the
`aws.events`→Sync fallback), the SSM-gated webhook shape (`aws-ssm.tf` `discord_webhook`: SecureString
seeded `UNSET`, `ignore_changes`, "UNSET reads as webhooks-off" — the second param is a copy of a
precedented thing, not a novelty), the single-table item conventions, the `ping_msg` chunk/deliver
machinery, and the record→act→mark orchestration proven by `choosecontent`.

## selection — the candidate predicate (v1, family-reviewed)

A whisperable treasure is a game where **all** of:

1. **`is_listable()`** — `status == Available && giftable && !hidden` (domain's own gate). This
   already excludes: claimed/pending (in flight), gifted, ben-redeemed (self-claim = he knows it
   exists — deliberate, per family consensus), expired, and hidden.
2. **Not promised on an active curated link.** OMBB's rule — *"unspent ≠ unpromised"*: a link cut
   Tuesday and redeemed in three weeks must not be whispered in the window. Refined against the
   schema, because the naive form is the vacuous-exclusion trap: `Link.curated_game_ids: None` =
   **open shelf = the whole listable catalog** — measured **18 of 18 links open-shelf, 0 curated,
   as of 2026-08-28** (a census is a script, not a paragraph — re-derive:
   `aws dynamodb scan --table-name brd-prod-ue1-bendobundles-table --filter-expression
   'begins_with(pk, :l) AND sk = :m' --expression-attribute-values
   '{":l":{"S":"LINK#"},":m":{"S":"META"}}' --projection-expression 'pk, curated_game_ids'` and
   count the items lacking `curated_game_ids`). The RULE survives any number; the number needs a
   date. Excluding every game "offered on a link" would empty the attic forever. **A curated pick is a promise; an open shelf is
   not.** Exclusion set = ⋃ `curated_game_ids` over links that are ACTIVE: `!revoked` ∧
   (`expires_at` absent or future) ∧ `claims_used < claims_allowed`. (A sealed link is active —
   the promise is made before the unwrap.)
3. **Not already whispered this cycle** — excluded by whisper-log items with `delivered = true`
   for the current cycle. **An undelivered log item is a failure receipt, never an exclusion**
   (OMBB's ①×two-write catch: anything else burns treasures on a dark deploy's happy path).

Ranking: candidates with `artwork_url` outrank artless; artless still eligible when they're all
that's left (delight never gates — PRODUCT.md principle 5). Pick: deterministic index over the
title-sorted pool — `(julian_day × 2654435761) mod len`. ⚠️ Scope of that determinism, said out
loud (Lilith): it re-derives the same winner **only over an unchanged pool** — a claim landing
between attempts remaps the index. The conditional put is the guarantee; the hash is a belt. And
**coverage across a cycle is carried by the exclusion log, never by the hash** — do not "improve"
the distribution believing it owns coverage; it doesn't and it doesn't need to.

**Exhaustion (both reviewers, OMBB's words kept):** *corpus size IS the period, and silence reads
as broken.* When the pool for the current cycle empties, the cycle number increments and the attic
starts over — a whisper twice a year is a feature; quiet forever is the bug this exists to kill.
Pool size (N) is logged on every run, so the period is a measured number, not a guess.

**The population truth (Lilith + OMBB, the joint finding):** never-keyed treasures and
gifted-out-of-band treasures are one problem from two ends — **no enumeration gets both right, so
an explicit manual retire is required regardless.** It already exists: `Store::set_game_hidden`;
hidden fails `is_listable()`. The whisper inherits it rather than inventing one.

## idempotence, dark deploys & failure honesty

EventBridge can double-fire and lambdas restart mid-flight; the **act** (a Discord message) and the
**record** (the log item) are two writes.

- 🔴 **The dark-deploy rule (OMBB, the round's best catch):** if the whisper webhook resolves to
  anything but a live URL, the run is a **loud no-op with ZERO writes** — no selection recorded, no
  log item, nothing marked. `param absent ⇒ skip send, do NOT mark` is an explicit tested arm.
  Inert is fine; inert-and-marking is data loss.
  🔴 **And dark itself has two faces (OMBB, gate 5, MAJOR 1):** `Notify` is three-state —
  `Webhook / Disabled / Unresolved` — and `Unresolved` means *configured but UNREADABLE* (IAM/KMS/
  read-path fault). Telling Ben `put-parameter --overwrite` there is wrong and DESTRUCTIVE: the
  stored value may be right and overwriting replaces a good secret while fixing nothing. The dark
  arm matches all three: `Disabled` → the light-it one-liner · `Unresolved` → "configured but
  UNREADABLE — check ssm:GetParameter + the KMS grant". Distinct wording, both tested.
- 🔴 **The whisper's off-switch is its OWN (OMBB, gate 5, MAJOR 2):** the ops `Notify` resolves
  under the global `NOTIFY_DISABLED`; reusing that flag for the whisper would re-couple the
  registers the second param exists to separate — quieting ops would silently kill the gift
  feature. The whisper `Notify` resolves under `WHISPER_DISABLED`, and a tested arm asserts
  `NOTIFY_DISABLED=1` does NOT dark the whisper.
- **The dark state announces itself (Lilith):** that same arm sends ONE line to the *existing ops
  webhook*: whisper is dark + the exact `aws ssm put-parameter` one-liner to light it. The ops
  register doing what the ops register is for; the whisper register stays warm-only.
- The whisper-log item is put **conditionally** on `attribute_not_exists(pk)`, keyed by the
  **slot identity: the ISO week** (`WHISPER#2026-W35`). Not the UTC date — the schedule speaks
  America/New_York and a date key speaks UTC, and *two clocks disagreeing about what "a day" is*
  means a retry crossing UTC midnight would mint a fresh key and double-send (Lilith, round 4 —
  her own timezone recommendation created the mismatch and she flagged it). The ISO week is stable
  across the whole weekend in either zone, and it IS the tick's identity for a weekly cadence.
  ⚠️ NAMED COUPLING: the key grain equals the cadence grain — a future sub-weekly schedule must
  change the slot derivation in the same commit, or ticks collide silently. And the margin is a
  property of the SLOT, not the key (Lilith): Saturday morning sits ~38h from the ISO-week
  boundary, but a Sunday-evening ET run computes into the NEXT ISO week in UTC (Sun 20:00 ET =
  Mon 01:00 UTC) — anyone moving the cadence re-checks the boundary distance. Only the winner sends;
  a loser exits quietly — that slot's whisper already belongs to someone.
- Order is record → send → mark-delivered. A crash between record and send loses at most one
  whisper (that date's item sits `delivered=false` — visible as exactly what it is — and the GAME
  stays eligible next tick, because exclusions read `delivered=true` only). It can never
  double-send.
- Whisper errors are their own: `Whisper` is its own match arm; a selection bug must never touch
  sync.
- **Fail distinct, not fail quiet (OMBB, round 2 — after his own bad predicate would have built a
  permanently-empty pool wearing a quiet week's face):** a no-send has distinct causes and each
  says WHICH, one line, distinct wording: ① **dark param** → the ops-register one-liner above ·
  ② **empty-by-predicate** (zero candidates even after cycle rollover — the vacuous-predicate
  case) → its own ops-register line naming the pool sizes at each filter stage, because *"a
  vacuous predicate looks exactly like a well-behaved quiet week"* · ③ **conditional-put loser**
  (this date already whispered) → log-only, benign by design · ④ **send failed** → the
  `delivered=false` receipt AND **its own ops-register line — REQUIRED, not optional (Lilith,
  gate-5 round, structurally forced):** the never-ran alarm is BLIND to ④ by construction — the
  invocation fired, the lambda ran, the target didn't error, so the bucket has data and reads
  healthy, and `TargetErrorCount` stays 0. A failed send pings nothing else, won't retry until
  next week's slot, and Ben's only remaining detector would be *noticing he didn't get a whisper —
  the exact faculty this feature exists because he lacks.* The thesis eats itself without this
  line. Each cause is a tested arm.
- **Cause ⑤ is structurally out of the mechanism's reach (Lilith, round 3): the run that never
  happened** — schedule not firing, cold-start crash, lapsed permission. *"A monitor whose alert
  path runs through the monitored channel reports healthy and silent identically"* — the no-send
  announcement rides the thing being announced. So ⑤ gets an instrument OUTSIDE the whisper: a
  CloudWatch alarm in the repo's existing `aws-cloudwatch-alarms.tf` on the Scheduler's own
  invocation metrics (`AWS/Scheduler` `InvocationAttemptCount` < 1, missing data treated as
  breaching, plus `TargetErrorCount > 0`) — a different trigger, so it cannot inherit the failure
  it watches for. 🔴 AMENDED AT FIRST APPLY (2026-08-28): AWS refuses evaluation windows over one
  week ("Metrics cannot be checked across more than a week" — the 8-day design had a day of slack
  the platform does not sell, found by the API, not the reviews). Under the 7-day cap a weekly
  tick has ZERO slack (the window and the spacing are equal, so ingestion lag false-fires weekly).
  ⇒ **The slack moved from the ALARM into the SCHEDULE: a Sunday HEARTBEAT tick**
  (`cron(0 10 ? * SAT,SUN *)`). Saturday always wins the ISO-week slot (it precedes Sunday in an
  ISO week), so Sunday exits as the DESIGNED cause-③ quiet loser — it keeps the invocation metric
  present at ≤6-day gaps under a 7×daily alarm (one full day of slack), exercises the loser path
  weekly, and doubles as the retry day if a Saturday tick outright fails. The cadence Ben sees is
  unchanged: whispers happen on Saturdays (Sunday can only ever whisper if Saturday's tick never
  ran — a self-heal, not a second whisper).

## the message itself

Register: the friend-surface voice — lowercase, cozy, ♡ — NOT the operator-alert voice. Plain
markdown content through the existing chunk/deliver machinery (Discord unfurls the art URL; no
embed API needed in v1):

> 🕯️ *from the attic…*
> **{title}** has been sleeping in *{bundle}*.
> cut a link for someone ♡ → {site}/admin/catalog?q={title urlencoded}
> {artwork_url}

(No "waiting since {year}" — measured: `Game` carries no acquisition date; the bundle name carries
the nostalgia instead. The deep-link needs zero web changes: `Catalog.tsx` already reads `?q=`.)

## config

- `aws_scheduler_schedule` + `schedule_expression_timezone = "America/New_York"`, default
  `cron(0 10 ? * SAT *)` — a human Saturday 10:00 that survives DST (classic EventBridge rules are
  UTC-only; corroborated by both reviewers). New resource type + a scheduler execution role scoped
  to `lambda:InvokeFunction` on fulfillment.
- `whisper_webhook` SSM SecureString, exact `discord_webhook` shape: seeded `UNSET`,
  `ignore_changes = [value]`, value set out of band, never through terraform. Second param, not
  shared — **push fails silent where pull fails loud** (OMBB): sharing one webhook means an
  ops-side rotation kills whispers for a month before anyone notices.
- Env: `WHISPER_WEBHOOK_PARAM`, `WHISPER_SITE_URL` (default `https://bendobundles.com`).

## non-goals (v1)

No email. No in-app whisper surface. No unsubscribe UI (the tf enable flag is the off switch and
it's Ben's). No batching. No quality scoring / genre rotation / seasonal theming — the whisper's
charm is that it's the attic's voice, not a recommender system.

## family review log (round 1, 2026-08-28)

- OMBB: second param for **failure asymmetry** (not register) · the ①×two-write dark-deploy data
  loss · issued-not-spent exclusion · gifted-out-of-band needs manual retire · *corpus size IS the
  period*. Taken, with issuance refined to curated-only (the open-shelf measurement).
- Lilith: dark state must announce itself via ops register · `aws_scheduler_schedule` for real
  timezones · population must be treasures, not links (confirmed by measurement: it is) ·
  partial-spend and pending/compensated questions (answered by measurement: n/a at game level;
  status transitions already correct) · corroboration-vs-shared-inference split on the joint calls.
- Joint reasoning, logged as a **confirmation, not a catch** (Lilith's correction to the record):
  "no enumeration handles out-of-band both ways ⇒ explicit manual retire regardless" — and the
  retire **already exists** (`set_game_hidden`); the whisper inherits it. Two reviewers reasoned
  their way to a thing already built, which confirms the build rather than discovering a gap.
- OMBB round 2: **fail distinct, not fail quiet** — the no-send causes must not share a face
  (absorbed into the failure-honesty section, each cause a tested arm).

## verification (pounce steps 11–13)

Terraform applies via my deploy role (AWS_PROFILE explicit, per the box-identity rule). Live-fire:
invoke the lambda once with `{"op":"whisper"}` off-schedule against prod. Expected on a dark
deploy: the loud no-op line, the ops-webhook one-liner, and **zero** `WHISPER#` items — that's the
tested arm running live. Then hand Ben the put-parameter one-liner at reveal; the first lit whisper
is the deploy verification's second half (read back the log item `delivered=true` AND see the
message with my own eyes in his channel — I can't; Ben confirms, or I verify the webhook POST 2xx
in the log line + item state).
