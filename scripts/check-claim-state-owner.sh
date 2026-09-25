#!/usr/bin/env bash
# The claim STATE MACHINE has one owner: fulfillment. admin-api may hold Arc<Store> and may CREATE a
# claim (claim_game_self — intended, 1 live call site), but it must never move one out of Pending and
# must not run the stuck-claims read. With the deployed policy unconditioned on the table, IAM will NOT
# stop it — this design boundary is the SOLE guard.
set -uo pipefail
SRC="${1:-crates/admin-api/src/lib.rs}"
# ANCHORED ON THE METHOD, NEVER THE RECEIVER: a receiver rebind (`let st = &s.store`) breaks a
# `store\.`-anchored adjacency while leaving the call untouched. Receiver names rebind; method
# names do not.
CLAIMSTATE='\b(compensate_claim|compensate_self_claim|fulfill_claim|list_pending_claims)\s*\('
[ -r "$SRC" ] || { echo "NOT MEASURED — cannot read $SRC"; exit 2; }
# DENOMINATOR = every method-call site, because that is the population this predicate filters.
# `[.:]` NOT `\.` — a leading-dot-only denominator does NOT contain a UFCS breach
# (`Store::compensate_self_claim(&s.store, ..)`), which the predicate DOES flag, making the
# numerator a non-subset of its denominator.
total=$(grep -oE '[.:][a-z_][a-z0-9_]*[[:space:]]*\(' "$SRC" | wc -l)
hits=$(grep -nE "$CLAIMSTATE" "$SRC"); rc=$?
echo "denominator: $total method-call site(s) in $SRC, each judged against the claim-state set"
case "$rc" in
  1) echo "BOUNDARY HELD — 0 claim-state transitions in admin-api (claim CREATION is allowed and out of scope)"; exit 0 ;;
  0) echo "BOUNDARY BREACHED — admin-api moves claim state:"; echo "$hits"; exit 1 ;;
  *) echo "NOT MEASURED — grep rc=$rc"; exit 2 ;;
esac
