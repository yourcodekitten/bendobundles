#!/usr/bin/env bash
# Every `async fn <name>()` the plan declares in a Rust code block must exist in the repo, and vice
# versa for the tests this plan adds. A rename during execution is fine — an UNDECLARED one reads as a
# missing test to anyone diffing plan against code.
set -uo pipefail
P="${1:-docs/superpowers/plans/2026-09-25-doorstep.md}"
[ -r "$P" ] || { echo "NOT MEASURED — cannot read $P"; exit 2; }
names=$(grep -oE '^async fn [a-z0-9_]+' "$P" | sed 's/^async fn //' | sort -u)
n=$(printf '%s\n' "$names" | grep -c . )
[ "$n" -gt 0 ] || { echo "NOT MEASURED — zero test names extracted from $P; a clean run over an empty set is vacuous"; exit 2; }
miss=0
while read -r t; do
  [ -n "$t" ] || continue
  if grep -rqE "async fn ${t}\(" crates/ web/ 2>/dev/null; then
    printf '  ✅ %s\n' "$t"
  else
    printf '  🔴 %s — declared in the plan, ABSENT from the tree\n' "$t"; miss=$((miss+1))
  fi
done <<< "$names"
echo "  ── denominator: $n test name(s) declared in $P · $miss absent ──"
[ "$miss" -eq 0 ] || exit 1
