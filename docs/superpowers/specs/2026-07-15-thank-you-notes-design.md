# thank-you notes — the friend's word back to ben

**date:** 2026-07-15 · **author:** code kitten · **status:** implemented (same-day, kitten-initiated)

## why

#69 gave ben's gift note a ride TO the friend; nothing travels back. the product spec says
"an unclaimed game is a gift nobody got to open" — the same is true of an unread thank-you.
this closes the gift loop: after a friend claims, they get one soft, optional chance to say
thanks, and it lands where ben curates (admin → links). the unwrap stays the product; this
is the echo after it.

## shape (mirror of gift_note, deliberately)

- **link-level, not claim-level.** the link IS the friend's identity in this system; the
  gift note rides the link, so the thanks does too. one note per link, write-once.
- **storage:** top-level dynamo attrs `thank_note` (S) + `thanked_at` (N, epoch seconds —
  `epoch_s`'s blanket rule for top-level times) on `LINK#<token>/META`. written ONLY by
  `Store::set_link_thanks`, a scoped conditional `UpdateItem` — never through
  `update_link_meta`, never in `body` (`schema::link_body` strips both, same
  no-copy-at-rest rule OMBB set in #69). the condition storage-enforces the full guard
  ladder: `attribute_exists(pk) AND attribute_not_exists(thank_note) AND revoked = :f
  AND claims_used >= :one AND (attribute_not_exists(expires_at) OR expires_at > :now)`
  — write-once, dead links take no mail even racing a revoke, and the compensate
  window (claims_used is not monotonic) can't leave a note on a zero-claim link.
- **read:** `link_from_item` overrides both unconditionally from the top-level attrs
  (absence = never thanked), same as `gift_note`.

## api

### `POST /api/l/:token/thanks` (public-api, friend trust boundary)

body `{"note": string}`. guards, in order:

1. validate: trim; empty → 422; > 500 chars (`THANK_NOTE_MAX_CHARS`, same budget as
   `GIFT_NOTE_MAX_CHARS`) → 422. `{"error": msg}` shape.
2. unknown token → `link_not_found_response()` (byte-identical, no enumeration oracle).
3. link state: **revoked / expired → 409** (dead links don't take mail; same messages as
   the claim handler). active and **exhausted both accept** — a fully-claimed link is
   exactly when a friend says thanks.
4. `claims_used == 0` → 409 `"claim a game first"` — thanks is the echo of an unwrap,
   not a guestbook.
5. `set_link_thanks` → `AlreadyThanked` → 409 `"thanks already sent"`; success → 200
   `{"thank_note", "thanked_at"}` (canonical stored values).

CCF on the conditional write is classified ATOMICALLY from the failed write's own
`ReturnValuesOnConditionCheckFailure::AllOld` item, in this order: missing → 404,
revoked → 409 revoked, expired → 409 expired, note present → 409 already-sent, zero
claims → 409 claim-first. liveness (BOTH halves) before already-sent, byte-matching the
handler's pre-check ladder so the same link state gets the same message on either path
(pass-2 find). no follow-up read: an eventually-consistent re-read racing a concurrent
tab's write could misclassify AlreadyThanked as something scarier (pass-1 find).

### reads

- `LinkView.thank_note` (friend): gated by `hide_games` exactly like `gift_note` — a dead
  link serves neither direction of the correspondence.
- `AdminLinkView.thank_note` + `thanked_at` (admin): read-only. no admin writer — ben
  can't edit a friend's words, only receive them.

## web

- **friend (LinkPage):** a soft "say thanks to ben ♡" card, shown only when the link has
  claims, state is active/exhausted, and no note has been sent. textarea, 500-char budget
  with counter, one send. after sending (or on revisit) the card becomes the sent note —
  quiet confirmation, no ceremony theft from the unwrap. errors speak softly inline.
  reduced-motion safe (no animation gates the flow).
- **admin (Links):** links carrying a thank note show it as a quoted line + timestamp.
  this is ben's payoff surface — warm, but workbench-idiom.

## non-goals

- no edit/delete of a sent note (write-once keeps it honest; ben can be asked out-of-band).
- no notification pipeline (admin surface only, for now — a future enhancement could ping
  discord, but the plumbing stays invisible until then).
- no claim-level notes, no threading, no replies. it's a thank-you, not a chat.
