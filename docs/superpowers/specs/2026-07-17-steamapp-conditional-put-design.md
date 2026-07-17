# STEAMAPP# conditional put — close the lost-update by construction

**date:** 2026-07-17 · **author:** code kitten · **status:** draft · **issue:** #75

## why

#73's just-in-time re-read narrowed the STEAMAPP# lost-update window from run-length to
per-item seconds, but `put_steam_app` is an unconditional `PutItem`: a concurrent writer
landing inside the re-read→put gap (pace sleep + Steam RTT, seconds wide) is silently
overwritten. Lilith's #73 finding, converged with OMBB's read. Cache-not-safety —
provenance (GAME#) is untouched and damage self-heals on TTL windows — but "self-heals
in 14–30 days" is a euphemism for "wrong for 14–30 days." This closes it by
construction: the put succeeds only if the item is unchanged since the read that seeded
the merge; a lost race is detected, re-merged, and retried instead of silently clobbering.

## the race, concretely

Two writers exist, both in the fulfillment crate:

- **enrichment** (`run_steam_enrichment`, cron sync): JIT re-read → paced Steam calls
  (detail and/or reviews) → merge onto snapshot → unconditional put.
- **backfill** (`backfill_steam_details`, manual bin): same shape — JIT re-read → paced
  detail fetch → merge → unconditional put.

Both carry EVERY half of the cache in the write (merge write: partial progress survives
aborts). So if backfill's put lands while enrichment is mid-Steam-call for the same
app_id, enrichment's put then rewrites the whole item from its pre-backfill snapshot —
backfill's fresh detail is gone, clocks regress, and nothing logs. The
don't-run-backfill-during-cron rule covers the realistic overlap today; this makes the
rule unnecessary rather than load-bearing.

admin-api and public-api only **read** STEAMAPP# (`get_steam_app`,
`batch_get_steam_apps`). Test code seeds via `put_steam_app`. No other writers.

## design

### storage: a monotonic `version` attribute (optimistic lock)

`STEAMAPP#<app_id>/META` today is `{pk, sk, body}` where `body` is the whole
`SteamAppCache` as one JSON string — a `ConditionExpression` cannot see inside it. So
the item gains ONE top-level attribute:

- `version` (N) — monotonic write counter, starts at 1, +1 per successful put.

**Why a counter and not the clocks** (first draft used `fetched_at`/`reviews_fetched_at`
as the guard; OMBB + Lilith review, 2026-07-17): timestamps make a mushy version token —
two writers stamping in the same clock tick produce equal stamps and the race sails
through undetected; the guard silently degrades to hope exactly under load. A monotonic
integer is collision-proof by construction, and a single attribute also dissolves the
half-migrated-item problem (with two guard attrs, any path that could ever write one
without the other creates items the condition misjudges; with one attr there is no
"half"). The clocks stay where they are — inside `body`, honest freshness data, not
enforcement. No top-level clock duplicates are added (YAGNI; nothing reads them).

**Invariant (recorded per Lilith — a decision, not an accident):** `version` and `body`
travel together, always, atomically. `steam_app_item` is the only builder of this item
shape and takes the version as a parameter of the same call that serializes `body`;
`Store::put_steam_app` is the only STEAMAPP# writer and computes the written version
from the guard (`Absent`/legacy → 1, `Unchanged(v)` → v+1) in the same `PutItem` that
carries `body`. No `UpdateItem` ever touches these items; no code path can move one
without the other. Per the `link_from_item` doctrine: top-level attr for enforcement,
`body` for parsing — `parse_body::<SteamAppCache>`, the struct, and the `body` wire
format are all unchanged.

Reads used by admin-api/public-api (`get_steam_app`, `batch_get_steam_apps`) are
untouched — readers never see or need `version`.

### store API: guard is mandatory (compile-breaking, deliberately)

```rust
/// Opaque optimistic-lock token for a STEAMAPP# item — obtainable ONLY from
/// `get_steam_app_versioned` (private field: a guard value cannot be fabricated,
/// it must come from a read). `None` inside = legacy item written before the
/// version attribute existed.
pub struct SteamAppVersion(Option<i64>);

/// What the caller read before merging — the precondition for the write.
pub enum SteamAppPutGuard {
    /// The read returned None: create-only.
    Absent,
    /// The read returned an item carrying this version token: write only if
    /// the item is still at that version.
    Unchanged(SteamAppVersion),
}

/// The writer-side read: snapshot + its version token.
pub async fn get_steam_app_versioned(
    &self,
    app_id: u32,
) -> Result<Option<(SteamAppCache, SteamAppVersion)>, StoreError>;

pub async fn put_steam_app(
    &self,
    cache: &SteamAppCache,
    guard: SteamAppPutGuard,
) -> Result<(), SteamAppPutError>;
```

`put_steam_app` keeps its name but changes signature — every call site (two prod, ~30
test) breaks at compile and must state its precondition. No unguarded variant survives:
an unconditional escape hatch would be the next silent clobber waiting for a caller.
Test seeding uses `Absent` (fresh tables); overwrite sequences read the token back via
`get_steam_app_versioned` first — the same discipline prod follows.

Condition expressions and written version:

- `Absent` → `attribute_not_exists(pk)`; writes `version = 1`.
- `Unchanged(None)` (legacy item) → `attribute_not_exists(version)`; writes
  `version = 1`. This is the **migration arm**: pre-change items have no `version`,
  and the first guarded write adopts them. It cannot false-pass — any concurrent
  new-code write stamps `version`, which flips the arm to a CCF.
- `Unchanged(Some(v))` → `version = :v`; writes `version = v + 1`.

(Considered and rejected: conditioning on `body = :old_body` string equality —
byte-exact, but hauls a potentially-tens-of-KB comparand in every put, and makes JSON
key order and float formatting correctness inputs. Also rejected: clock-pair guard —
see above.)

### error: a distinguishable lost-race signal

```rust
#[derive(Debug, thiserror::Error)]
pub enum SteamAppPutError {
    /// The item changed between the caller's read and this put — a concurrent
    /// writer won. Nothing is wrong with the payload; re-read, re-merge, retry.
    #[error("STEAMAPP# item changed since read — lost the race, safe to re-merge")]
    LostRace,
    #[error(transparent)]
    Store(#[from] StoreError),
}
```

Follows the `ClaimTxError::TxConflict` precedent (a timing race is not an AWS error);
detection reuses the existing `is_ccf_put` predicate. Adding a variant to `StoreError`
itself was rejected: every `StoreError` consumer in three crates would need an
unreachable arm for an error only this call can produce.

### callers: re-merge once, then yield the pass

Both callers get the same policy. On `LostRace`:

1. **Re-read** the item via `get_steam_app_versioned` (one extra GetItem, only on the
   rare conflict path) — fresh snapshot + fresh token.
2. **Re-merge per half, newest-wins:** apply OUR detail half (detail + `fetched_at` +
   delisted stamps) onto the fresh snapshot only if our `fetched_at` ≥ theirs; same
   independently for the reviews half (`overall` + `recent` + `reviews_fetched_at`).
   The Steam data is already in hand — re-merging costs **zero** storefront calls, so
   the retry cannot amplify rate-limit pressure. If the concurrent writer's half is
   newer, theirs survives — that is the correct outcome, not a loss.
3. **Retry the put once** with a guard from the fresh snapshot.
4. A second `LostRace` (two concurrent writers hitting the same item twice within
   seconds) → warn with a distinct message and skip the app; the next sync/backfill
   pass retries naturally. No loops, no unbounded retries.

The re-merge is extracted as a **pure function** in the fulfillment crate —
`merge_steam_halves(snapshot, ours) -> SteamAppCache` (exact name/shape at plan time) —
so the newest-wins-per-half policy is unit-testable without simulating a live race, and
enrichment and backfill share one definition of it (the `SteamAppCache::empty` rule:
shared semantics get one home).

Observability: the enrichment summary line and `BackfillSummary` each get a
`lost_race` counter (races that were detected and re-merged — the metric that proves
the guard is earning its keep), distinct from `failed`.

## non-goals / out of scope

- GAME# provenance writes — unaffected, different item class, already transactional
  where it matters.
- DynamoDB TTL for STEAMAPP# — separate concern, unchanged.
- `batch_get_steam_apps` projection (#64), `as_wire` coverage (#74) — separate issues.
- Retiring the don't-run-backfill-during-cron **habit** — the guard makes it
  non-load-bearing, but running them apart remains polite (less CCF churn, less Steam
  pacing contention). Doc note, not code.

## tests

- **store (dynamo crate, against local dynamo):** `Absent` on empty table succeeds and
  writes `version = 1` (assert via raw client); `Absent` over an existing item →
  `LostRace`; `Unchanged` with the current token succeeds and increments `version`;
  `Unchanged` with a stale token → `LostRace`; **legacy item** (seeded raw
  `{pk, sk, body}` with no `version`, via the existing `raw_client` helper) reads back
  a legacy token, accepts an `Unchanged` write through the migration arm (item now at
  `version = 1`), and re-using that same legacy token afterward → `LostRace`.
- **merge policy (fulfillment, pure):** ours-newer / theirs-newer / equal-clock per
  half, delisted-stub vs live-detail collisions, empty-snapshot cases.
- **caller race path (fulfillment handler tests):** deterministic race via the mock
  Steam client — its `get_app_details` hook writes a conflicting STEAMAPP# item to the
  store mid-call (the JIT re-read has already happened), forcing the put into
  `LostRace` and asserting the re-merged result contains BOTH the concurrent writer's
  half and ours, plus the `lost_race` counter. (Plan verifies the mock supports a
  store-writing hook; if not, the mock grows one — it's test code.)
- **existing suites:** all current `put_steam_app` call sites updated mechanically;
  behavior of every existing test preserved.

## deploy

No data migration (the legacy arm adopts items on their first guarded write). Ships as
the standard three-lambda deploy; only the fulfillment lambda's behavior changes
(admin/public link against the dynamo crate but never write STEAMAPP#). Rollback-safe
both directions: `PutItem` replaces the whole item, so a rolled-back (old-code) put
simply reverts the item to the legacy `{pk, sk, body}` shape — no stale `version` can
strand — and the migration arm re-adopts it on the next new-code write. Old readers
never see the new attribute (`parse_body` reads `body` only). The only cost of a
rollback is losing the guard itself, which is exactly today's behavior.
