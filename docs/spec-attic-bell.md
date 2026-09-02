# the attic bell 🔔 — spec

*2026-09-02, kitten. Status: DRAFT → family review (OMBB + Lilith) → plan.*

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
- **Unwrap bell fires from INSIDE fulfillment's Gift success path** — it is already the
  post-durable-write point, in the process that owns the transport; no extra invoke exists to
  fail. Link label comes from a store read it can already do.
- **Thanks bell**: public-api owns `set_link_thanks`, so after the durable write it invokes
  fulfillment with the `Bell` op **fire-and-forget** (`InvocationType::Event` — a new invoker
  method beside the existing RequestResponse `gift()`; the trait grows, the Gift path does not
  change).
- **Failure disposition: best-effort, loud in logs, invisible to friends.** A bell failure must
  never fail, slow, or color the claim/thanks response — *delight never gates*, and the bell is
  delight. Send failure = WARN with correlator, nothing else. Stated plainly: **the bell may
  miss; the gift may never.**
- **Dark-deploy doctrine inherited verbatim** from spec-attic-whispers: webhook param resolving
  `UNSET` ⇒ loud no-op log line, zero writes, zero friend-visible effect.
- **At-most-once, by disposition.** Async lambda invokes retry on *function error* — so the Bell
  handler swallows its own failures (logs, returns success) rather than signaling error. A missed
  bell is accepted; a double ring is cheap but sloppy; a retry storm is neither.

## content & security

- **Friend-triggerable sends are bounded by construction**: claims ≤ `claims_allowed` per link;
  thank-you is write-once per link (conditional update). No unbounded webhook-spam path exists.
- The thanks bell sends the **STORED** note — already control/bidi-sanitized at write
  (`sanitize_note`), already budgeted (`THANK_NOTE_MAX_CHARS = 500`) — never the raw request body.
- `allowed_mentions: {"parse": []}` on every bell (the #176 adjudication: denies notify; embeds
  unaffected).
- No friend PII beyond the link label, which ben himself wrote.

## open questions (for OMBB + Lilith before the plan)

1. **Webhook: reuse `whisper_webhook` or mint a third `bell_webhook` param?** Same audience, same
   register, same room argues reuse; the push-fails-silent doctrine that split whisper from ops
   argues every distinct *sender class* gets its own param. My lean: reuse — bell and whisper are
   one register (warm, ben-facing) and one room, and a param nobody sets is a bell nobody hears.
2. **Unwrap bell placement**: inside fulfillment's Gift success path (my lean, post-durable and
   free) vs a public-api-side post-response invoke (uniform with thanks, but adds an invoke that
   can fail and knows less).
3. **Choice claims** spend a monthly pick — same bell, or a variant line that says so? My lean:
   same bell, maybe "⟨title⟩ (a monthly pick, spent with love)".
4. **Double-ring tolerance**: is swallow-own-errors (at-most-once-ish) the right trade vs letting
   the default 2-retry stand (at-least-once, possible double ring)? My lean: swallow.
