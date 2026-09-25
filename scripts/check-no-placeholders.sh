#!/usr/bin/env bash
# No placeholder / no-panic guard over ADDED lines of a file's uncommitted diff, COMMENTS STRIPPED.
#
# 🔴 TWO DEFECTS THIS FILE ALREADY HAD, both found by controls rather than by reading:
#  ① it matched the RAW diff and went red on comments EXPLAINING why `.expect()` / `.ok().flatten()`
#    are not used — describe-vs-quote, inside the guard for it. Fixed by stripping `//` first.
#  ② an unreadable path, an untracked file, or a file with NO CHANGES all yielded zero added lines,
#    so grep found nothing, so it printed a ✅ — a GREEN OVER AN EMPTY POPULATION. A guard's
#    not-applicable IS its pass state, which is what makes a mis-scoped one invisible.
# ⇒ NOT APPLICABLE and CLEAN are now different verdicts with different exit codes.
set -uo pipefail
f="${1:-crates/fulfillment/src/lib.rs}"
[ -r "$f" ] || { echo "NOT MEASURED — cannot read $f"; exit 2; }
git ls-files --error-unmatch "$f" >/dev/null 2>&1 || { echo "NOT MEASURED — $f is not tracked; git diff cannot see it"; exit 2; }
added=$(git diff -U0 -- "$f" | grep '^+' | grep -v '^+++')
n=$(printf '%s' "$added" | grep -c . )
if [ "$n" -eq 0 ]; then
  echo "⬜ NOT APPLICABLE — $f has 0 added lines in the working diff. This is NOT a clean bill."
  exit 3
fi
code=$(printf '%s\n' "$added" | sed 's@//.*@@')
hits=$(printf '%s\n' "$code" | grep -nE 'REPLACE|placeholder|\.expect\(|\.ok\(\)\.flatten')
case "$?" in
  1) echo "✅ CLEAN — $n added line(s) in $f scanned, 0 forbidden constructs in CODE (comments excluded)"; exit 0 ;;
  0) echo "🔴 REFUSED — forbidden construct in added CODE of $f:"; echo "$hits"; exit 1 ;;
  *) echo "NOT MEASURED — grep failed"; exit 2 ;;
esac
