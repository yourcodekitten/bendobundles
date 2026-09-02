# the attic bell 🔔 — spec

*2026-09-02, kitten. Status: BUILT — family review (OMBB + Lilith, 8 rounds) → plan
`docs/superpowers/plans/2026-09-02-attic-bell.md` → OMBB sign-off at `21e5e9f` → implemented.
Where this document's narrative sections disagree with **decisions** below, the decisions win —
they are the reviewed outcome and the narrative is the proposal it started as.*

## why this exists

PRODUCT.md's success line is *"a friend opens their link and feels chosen for, the claim works on
the first try, and ben's library stops gathering dust."* Two of the three moments that close the
giving loop are **silent toward ben today**:

- **The unwrap.** A claim writes its CLAIM item, fulfillment mints the Humble gift link — and ben
  learns nothing unless he happens to open the admin. The emotional center of the product fires
  into the void.
- **The thank-you.** `thank_note` (#69's return path) lands write-once in dynamo and renders
  read-only on the admin Links page. Ben *"forgets his bundles exist 95% of the time"* — a
  friend's words can wait days unseen, which is the exact shape of the dust the product exists to
  clear.

The attic already knows how to speak to ben — the whisper webhook, once a week. The bell makes it
speak **at the moments that matter, when they happen**.

## what it is

Two bell events, delivered to ben's Discord in the attic voice, via the whisper-family transport:

1. **unwrap bell** — when a claim reaches durable success (gift URL written / choice spent and
   redeemed): a warm one-liner — *"🔔 ⟨link label⟩ just unwrapped ⟨game title⟩ ♡"* — with the
   game's cover art (`Game.artwork_url`) as an embed thumbnail and a deep link to the admin links
   page.
2. **thanks bell** — when a thank-you lands (`set_link_thanks` conditional write succeeds):
   *"💌 ⟨link label⟩ says: '⟨note⟩'"* — the friend's words reaching ben in the moment they were
   written, not on his next admin visit.

### non-goals (decided, not omitted)

- **No bell on link open / browse / seal-unlock.** A friend browsing must not page ben — the claim
  is the consent moment. (Also removes the noisiest, least meaningful event class.)
- **No bell on admin self-claim.** Ben does not need to be told what he just did.
- **No retroactive bells at deploy.** Existing claims/thanks stay silent; the bell rings forward.
- **Not a reliability channel.** #161 (per-row write failures invisible from discord) is a
  different register (ops, not warmth) and stays its own issue. The bell never carries errors.

## architecture

- **New `FulfillRequest::Bell { event }` op** in the fulfillment lambda. It shares whisper.rs's
  send machinery — the SSM-gated webhook, the `allowed_mentions: {"parse": []}` shape, the embed
  builder — and **NEVER touches `WHISPER#` slot state**. (The weekly slot invariant stands;
  Saturday 2026-W36's first full-card tick must arrive virgin.) One send function, two callers:
  a second implementation of the webhook POST is the drift this repo already refuses.
- **BOTH bells fire from public-api as fire-and-forget `InvocationType::Event` invokes** (a new
  `Invoker::bell` beside the RequestResponse `gift()`; the trait grows, the Gift path's behaviour
  does not). *This paragraph's first draft put the unwrap ring INSIDE fulfillment's Gift success
  path — see Q② below for why that lost: a webhook connect-hang would have sat in the friend's
  claim latency, and lambda-freeze leaves no "after the response" to hide it in.*
- **One exception, decided at sign-off: the RECONCILE HEAL rings INLINE**, from
  `claimed_tpk_terminal`'s guarded `GiftUrl` arm. A healed claim is a friend whose HTTP path never
  completed — a real unwrap ben was never told about — and reconcile is a background invocation,
  so nobody waits on that webhook. The arm is guarded on `link_token != SELF_LINK_TOKEN`: ben's
  own reconciled self-claim must not ring.
- **Failure disposition: best-effort, loud in logs, invisible to friends.** A bell failure must
  never fail, slow, or color the claim/thanks response — *delight never gates*, and the bell is
  delight. **The bell may miss; the gift may never.** A send failure is an ERROR log **and** an
  ops `ping_msg` on the other credential (Q④) — a WARN nobody reads is at-never-once.
- **Dark-deploy doctrine inherited verbatim** from spec-attic-whispers: webhook param resolving
  `UNSET` ⇒ loud no-op log line, zero writes, zero friend-visible effect. `BELL_DISABLED=1` is the
  bell's own separate mute (Q①).
- **Delivery honesty: at-most-once against HANDLER failure, at-least-once by DELIVERY.** See Q④ —
  the handler swallows its own errors so a retry cannot double-send after a partial success, but
  Lambda's async delivery contract still permits a rare duplicate, which is accepted.

## content & security

- **Friend-triggerable sends are bounded by construction**: claims ≤ `claims_allowed` per link;
  thank-you is write-once per link (conditional update). No unbounded webhook-spam path exists.
- The thanks bell sends the **STORED** note — already control/bidi-sanitized at write
  (`sanitize_note`), already budgeted (`THANK_NOTE_MAX_CHARS = 500`) — never the raw request body.
- `allowed_mentions: {"parse": []}` on every bell. **Scope corrected in review:** it is documented
  for `content` and says nothing contractual about embeds, so all friend-influenced text rides
  `content` and the question does not arise (see the fences).
- No friend PII beyond the link label, which ben himself wrote.

## decisions (2026-09-02, family review ×8 rounds + OMBB sign-off at plan `21e5e9f`)

**Q① webhook — REUSE `whisper_webhook`, but SPLIT the off-switch.** OMBB's own doctrine's axis is
credential *lifecycle*: bell and whisper share one room, one URL, one rotation event, so two
params would store one secret twice and the un-updated copy would fail silently at rotation. The
bell gets `BELL_DISABLED` (its own env flag, mirroring `WHISPER_DISABLED`'s register-decoupling
rule) so muting per-event bells never darks the weekly whisper.

**Q② placement — uniform public-api `InvocationType::Event` invoke for BOTH events, 2s budget.**
An inline POST in fulfillment's Gift path would put a webhook connect-hang in the claim's latency
path (the whisper client's timeout is 5s, sized for a cron and inherited by every path), and
lambda-freeze rules out post-response sends. The 2s budget is chosen against the COLD path — this
app is low-traffic, so a warm-measured 1s would concentrate misses on the first claim after a
quiet spell, which is the claim that matters. Durations log `outcome = ok | err | timeout` beside
`bell_invoke_ms`: a killed request and a 1998ms success write the same-shaped number, so an
unlabelled p99 reads the cap as data.

**Q③ choice claims — same bell, one extra clause** (`a monthly pick, spent with love`). NO
remaining-pick count: it is not in hand at ring time and adding a read for it is not worth it.

**Q④ delivery — at-most-once AGAINST HANDLER FAILURE; delivery is at-least-once.** The handler
never returns a function error (a retry re-runs the whole handler and can double-send after a
*partial* success, with no idempotency key on a webhook POST). Lambda's async delivery is itself
at-least-once, so a rare double ring stays possible and is accepted. A send failure is
`tracing::error!` **plus an ops `ping_msg`** — verified at `crates/fulfillment/src/lib.rs` to ride
`deps.notify`, the OPS credential, so the miss report survives a dead whisper webhook.

### fences carried out of the review

- **The bell is not the pick ledger.** The admin is truth for the monthly-pick budget; the bell
  may miss and must never be counted with.
- **Reading the count line, direction stated.** Delivery is at-least-once, so `rings` may legally
  EXCEED `unwraps` — `rings > unwraps` is a benign duplicate; **`rings < unwraps` is the suspect
  direction.** `rings` counts unwrap bells ONLY (one population with `unwraps`), each counted in
  the week its EVENT carries — stamped beside the gift response — so a week-boundary straddle is
  bounded to milliseconds. Residual ±1 remains possible; only a sustained or multi-unit gap is
  signal.
- **The counter covers unwrap bells only.** A thanks-bell outage is invisible to the
  `unwraps · rings` line by design (counting thanks would break the pairing) and is visible only
  via the per-miss ops ping.
- **The count line clips ~a day a week, both columns equally.** The Saturday whisper reads
  `BELL#<current week>`, so unwraps/rings landing after that read are printed by no card. The
  direction rule survives (both columns drop together); totals under-report, said here rather
  than discovered.
- **Coverage split.** The `unwraps · rings` line covers *bell broken, channel healthy* — it
  structurally CANNOT report a dead whisper webhook (the report would die with the channel).
  Dead-channel detection is the per-miss ops `ping_msg`, on a different credential. A metric +
  alarm remains the deferred second step, with this line as its reason.
- **The ops register's reader is PROVISIONAL.** Every detection path here is a *sender*; nobody
  has demonstrated a human reads the ops room from this seat (OMBB's measured 8-sent-0-received
  specimen on his own box). Deploy verification includes ben confirming one ops-register message
  reached eyes; until then "detection in hours" is a capability claim, not a property.
- **Mentions-deny provenance.** `allowed_mentions: {"parse": []}` is DOCUMENTED for `content`;
  embed behaviour is observed-not-contract. All friend-influenced text therefore rides `content`
  (the thanks card carries zero embeds), so the question does not arise.
- **Never an empty embed object.** An embed with no renderable field is a Discord 400, so an
  artless unwrap card sends `embeds: []` — the bell must not die precisely for the games without
  cover art.

### deploy note

**No new infra.** The bell rides the existing whisper webhook and the existing
public-api→fulfillment invoke grant at `terraform/aws-lambda.tf:182-188`
(`lambda:InvokeFunction` on the fulfillment ARN — `InvocationType` is a request parameter, not a
separate IAM action, so `Event` needs no grant change). Dark-deploy behaviour inherited from the
whisper: webhook `UNSET` ⇒ loud no-op, zero writes; `BELL_DISABLED=1` is the bell's own mute.
