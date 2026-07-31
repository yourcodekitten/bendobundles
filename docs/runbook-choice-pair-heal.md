# Choice duplicate-pair heal (one-time, spec Q5 / A6b)

Removes the legacy offered/tpk duplicate GAME rows (15 in the 2026-07-31 prod scan) that the
pre-D7 sync minted — e.g. `GK:mylittleuniverse` **and** `GK:mylittleuniverse_row_choice_steam`.
The D7 sync change (Task 8) stops minting new ones; this bin sweeps the existing floor.

**This is NOT delete-on-absence.** Every delete rests on positive dual evidence, re-derived from
the *live* order at execution time: the sibling's id derives to the offered id through the grammar,
the offered row exists and is flipped (`requires_choice == false`), AND the live order carries a
tpk matching the offered name. The order authorizes; the scan only schedules.

The delete lives ONLY in this operator bin (`Store::delete_game` is `#[cfg(feature = "heal")]`) —
the lambda/sync build cannot compile it. "Sync never deletes" is a compiler guarantee.

## Preconditions

- The D7 sync change (Task 8) is DEPLOYED and at least one sync has run — the flip must exist
  before the sweep (close the factory, then sweep the floor).
- You have AWS credentials (`AWS_PROFILE=kitten-maintenance`), the prod `TABLE_NAME`, and a live
  `HUMBLE_COOKIE` (the same session value the lambda uses; needed to re-verify each pair against
  the live order).

## Procedure

1. **Dry-run** (default — reads only, never deletes):
   ```
   TABLE_NAME=<table> HUMBLE_COOKIE=<sess> AWS_PROFILE=kitten-maintenance \
     cargo run -p fulfillment --features heal --bin heal_choice_pairs
   ```
   Read the printed pair list + per-pair verdict. Expect ~14 `HEAL` + 1 `SKIP` (mylittleuniverse,
   `claim-entangled` — its sibling is referenced by a still-pending claim).

2. **Execute** (deletes gate-passing siblings; each re-derives its evidence from the live order):
   ```
   TABLE_NAME=<table> HUMBLE_COOKIE=<sess> AWS_PROFILE=kitten-maintenance \
     cargo run -p fulfillment --features heal --bin heal_choice_pairs -- --execute
   ```

3. **A6b verify**: re-run the dry-run (step 1). Expect ZERO gate-eligible pairs remaining. Paste
   both the execute output and the post-verify dry-run into the PR/issue thread.

4. **mylittleuniverse**: after its pending claim resolves, re-run steps 1–3. If the gate still
   refuses, heal by hand with four eyes on it (confirm the offered row is flipped and the claim no
   longer references the sibling id before deleting).
