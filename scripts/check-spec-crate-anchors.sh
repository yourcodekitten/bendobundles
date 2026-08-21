#!/usr/bin/env bash
# Every crate version a spec/plan CITES must match what Cargo.lock RESOLVES.
#
# WHY: on 2026-08-21 a spec anchored to `aws-smithy-runtime-api-1.15.0` while the lockfile
# resolved 1.14.0. The 1.15.0 tree existed in ~/.cargo/registry as residue from an abandoned
# `cargo update --precise` experiment, so it read back as real. The struct shapes happened to
# match, so the conclusion survived and only the anchor was wrong — which is exactly the kind
# of defect that reads current forever. It then SURVIVED a revision whose entire subject was
# that section: the fix landed on the count and never reached the citation under it.
#
# `ls ~/.cargo/registry` is not a measurement of what you compile. The lockfile is.
#
# ⚠️ WRITING DOCS ABOUT A BAD ANCHOR: describe the version, do not quote it. This script scans
# docs/ and crates/, so prose that reproduces a stale `crate-x.y.z` to explain it is flagged as
# the defect it is describing. That is not a false positive to suppress — a grep cannot tell a
# citation from a comment about a citation, so the writing side has to carry the discipline.
# (Learned three separate times in this repo on 2026-08-21, twice in this very file's own
# development, and once by a pre-commit guard tripping on the prose that explained it.)
#
# rc: 0 = all anchors match, 1 = drift, 2 = NOT MEASURED (fail closed).
set -uo pipefail

LOCK="${LOCK:-Cargo.lock}"
shift_docs=("$@")
if [ ${#shift_docs[@]} -eq 0 ]; then
  # Docs AND source: a stale crate citation in a `//` comment reads exactly as current as one
  # in a spec. OMBB found `aws-sdk-dynamodb 1.116.0` cited at crates/dynamo/src/lib.rs:1150 with
  # the lock resolving 1.119.0 — invisible to a docs-only scan.
  mapfile -t shift_docs < <(
    { find docs/superpowers -name '*.md' -type f 2>/dev/null
      find crates -name '*.rs' -type f 2>/dev/null; } | sort -u)
fi

[ -r "$LOCK" ] || { echo "NOT MEASURED — cannot read $LOCK"; exit 2; }
[ ${#shift_docs[@]} -gt 0 ] || { echo "NOT MEASURED — no docs to check"; exit 2; }

# Vacuous-pass guard: if the lock has no packages we would pass every comparison trivially.
[ "$(grep -c '^name = ' "$LOCK")" -ge 50 ] || {
  echo "NOT MEASURED — $LOCK looks truncated"; exit 2; }

fail=0
checked=0
anchors_seen=0
for doc in "${shift_docs[@]}"; do
  [ -r "$doc" ] || continue
  # crate-name-X.Y.Z anchors, e.g. aws-smithy-runtime-api-1.15.0
  while read -r anchor; do
    [ -n "$anchor" ] || continue
    anchors_seen=$((anchors_seen+1))
    crate="${anchor%-*}"
    cited="${anchor##*-}"
    resolved=$(awk -v c="name = \"$crate\"" '
      $0==c {want=1; next}
      want && /^version = / {gsub(/version = |"/,""); print; want=0}
    ' "$LOCK")
    [ -n "$resolved" ] || continue          # crate not in this lock — not our anchor to police
    checked=$((checked+1))
    if ! grep -qx "$cited" <<< "$resolved"; then
      echo "🔴 ANCHOR DRIFT in $doc: cites ${crate}-${cited}, lock resolves $(tr '\n' ' ' <<< "$resolved")"
      fail=1
    fi
    # Two citation forms, both live in this tree:
    #   `crate-1.2.3`  (spec/plan prose, registry-dir style)
    #   `crate 1.2.3`  (source comments, e.g. "In aws-sdk-dynamodb 1.116.0 there is no ...")
  done < <( { grep -ohE '[a-z0-9_-]+-[0-9]+\.[0-9]+\.[0-9]+' "$doc"
              grep -ohE '[a-z0-9_-]+ [0-9]+\.[0-9]+\.[0-9]+' "$doc" | tr ' ' '-'; } | sort -u)
done

# Split deliberately: "we found no citations" and "we found citations but none named a crate the
# lock knows" are DIFFERENT failures — a moved/renamed corpus vs a pattern that stopped matching.
# Collapsing two causes into one sentence is how a printer ends up naming a cause nobody verified.
if [ "$checked" -eq 0 ]; then
  if [ "$anchors_seen" -eq 0 ]; then
    echo "NOT MEASURED — scanned ${#shift_docs[@]} file(s) and found NO crate-version citations at all."
    echo "  (corpus moved/renamed, or the citation pattern no longer matches anything)"
  else
    echo "NOT MEASURED — found $anchors_seen citation(s), but none named a crate present in $LOCK."
    echo "  (wrong lockfile for this tree, or every cited crate is external to it)"
  fi
  exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "✅ all $checked crate anchor(s) match Cargo.lock"
