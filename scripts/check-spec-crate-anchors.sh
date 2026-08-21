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
# rc: 0 = all anchors match, 1 = drift, 2 = NOT MEASURED (fail closed).
set -uo pipefail

LOCK="${LOCK:-Cargo.lock}"
shift_docs=("$@")
if [ ${#shift_docs[@]} -eq 0 ]; then
  mapfile -t shift_docs < <(find docs/superpowers -name '*.md' -type f 2>/dev/null)
fi

[ -r "$LOCK" ] || { echo "NOT MEASURED — cannot read $LOCK"; exit 2; }
[ ${#shift_docs[@]} -gt 0 ] || { echo "NOT MEASURED — no docs to check"; exit 2; }

# Vacuous-pass guard: if the lock has no packages we would pass every comparison trivially.
[ "$(grep -c '^name = ' "$LOCK")" -ge 50 ] || {
  echo "NOT MEASURED — $LOCK looks truncated"; exit 2; }

fail=0
checked=0
for doc in "${shift_docs[@]}"; do
  [ -r "$doc" ] || continue
  # crate-name-X.Y.Z anchors, e.g. aws-smithy-runtime-api-1.15.0
  while read -r anchor; do
    [ -n "$anchor" ] || continue
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
  done < <(grep -ohE '[a-z0-9_-]+-[0-9]+\.[0-9]+\.[0-9]+' "$doc" | sort -u)
done

[ "$checked" -gt 0 ] || { echo "NOT MEASURED — no anchor matched any crate in $LOCK"; exit 2; }
[ "$fail" -eq 0 ] || exit 1
echo "✅ all $checked crate anchor(s) match Cargo.lock"
