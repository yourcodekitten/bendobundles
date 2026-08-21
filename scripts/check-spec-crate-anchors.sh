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
corpus=("$@")
if [ ${#corpus[@]} -eq 0 ]; then
  # Docs AND source: a stale crate citation in a `//` comment reads exactly as current as one
  # in a spec. OMBB found `aws-sdk-dynamodb 1.116.0` cited at crates/dynamo/src/lib.rs:1150 with
  # the lock resolving 1.119.0 — invisible to a docs-only scan.
  mapfile -t corpus < <(
    { find docs/superpowers -name '*.md' -type f 2>/dev/null
      find crates -name '*.rs' -type f 2>/dev/null; } | sort -u)
fi

[ -r "$LOCK" ] || { echo "NOT MEASURED — cannot read $LOCK"; exit 2; }
[ ${#corpus[@]} -gt 0 ] || { echo "NOT MEASURED — no docs to check"; exit 2; }

# Vacuous-pass guard: if the lock has no packages we would pass every comparison trivially.
[ "$(grep -c '^name = ' "$LOCK")" -ge 50 ] || {
  echo "NOT MEASURED — $LOCK looks truncated"; exit 2; }

fail=0
checked=0
anchors_seen=0
skipped=0
for doc in "${corpus[@]}"; do
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
    # A skip is legitimate (the cited crate simply is not in this lock) but it is
    # INDISTINGUISHABLE from the extraction having failed for that crate. Counted and printed,
    # so a silent drop in coverage is visible instead of being something a reader would have to
    # remember the previous number to notice. This script exists to fight exactly that class.
    [ -n "$resolved" ] || { skipped=$((skipped+1)); continue; }
    checked=$((checked+1))
    if ! grep -qx "$cited" <<< "$resolved"; then
      echo "🔴 ANCHOR DRIFT in $doc: cites ${crate}-${cited}, lock resolves $(tr '\n' ' ' <<< "$resolved")"
      fail=1
    fi
    # TWO CITATION FORMS, AND THEY ARE NOT APPLIED TO THE SAME FILES — deliberately.
    #
    #   `crate-1.2.3`  registry-dir style. A strong signal. Matched EVERYWHERE.
    #   `crate 1.2.3`  space form. Matched in SOURCE ONLY.
    #
    # The space form cannot be applied to prose. Measured: a `.md` sentence like
    # "we removed rustls 0.21.12 from the tree" parses as a citation of `rustls` and is
    # reported as drift against the resolved 0.23 — a FALSE RED, produced by ordinary
    # English about a crate. And false reds are not harmless: they bury the true ones.
    # Retraction and changelog prose talks about old versions constantly, which is exactly
    # the writing this repo does most.
    # In a source comment the same shape IS a citation ("In aws-sdk-dynamodb 1.116.0 there
    # is no ..."), which is the real specimen this arm was added for, so it stays there.
    #
    # 📉 WHAT THIS COSTS, stated rather than left as a quietly smaller number: scoping the
    # space form to source dropped 4 real markdown citations from coverage — axum, lambda_http,
    # reqwest and tower (all matching the lock at the time of the change). A `.md` citation is
    # only verified if it uses the `crate-x.y.z` form. That is a deliberate trade: a false red
    # buries true ones, and these four were correct anyway. **If you widen this again, re-run
    # the prose control first** — `"we removed rustls 0.21.12"` in a `.md` must NOT go red.
  done < <( { grep -ohE '[a-z0-9_-]+-[0-9]+\.[0-9]+\.[0-9]+' "$doc"
              case "$doc" in
                *.rs) grep -ohE '[a-z0-9_-]+ [0-9]+\.[0-9]+\.[0-9]+' "$doc" | tr ' ' '-';;
              esac; } | sort -u)
done

# Split deliberately: "we found no citations" and "we found citations but none named a crate the
# lock knows" are DIFFERENT failures — a moved/renamed corpus vs a pattern that stopped matching.
# Collapsing two causes into one sentence is how a printer ends up naming a cause nobody verified.
if [ "$checked" -eq 0 ]; then
  if [ "$anchors_seen" -eq 0 ]; then
    echo "NOT MEASURED — scanned ${#corpus[@]} file(s) and found NO crate-version citations at all."
    echo "  (corpus moved/renamed, or the citation pattern no longer matches anything)"
  else
    echo "NOT MEASURED — found $anchors_seen citation(s), but none named a crate present in $LOCK."
    echo "  (wrong lockfile for this tree, or every cited crate is external to it)"
  fi
  exit 2
fi
[ "$fail" -eq 0 ] || exit 1
echo "✅ all $checked crate anchor(s) match Cargo.lock (${skipped} citation(s) skipped — not crates in this lock)"
