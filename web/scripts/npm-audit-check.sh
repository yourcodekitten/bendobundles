#!/usr/bin/env bash
# npm audit with a dated allowlist (#81) — npm has no per-advisory ignore, and the
# only "fix" it offers for the entry below is a DOWNGRADE. This fails on any
# production-dependency advisory NOT explicitly allowlisted here, so new ones
# still break CI while documented-unreachable ones don't bury the signal.
# NOTE: this is one of TWO advisory parking lots — rust advisories live in
# .cargo/audit.toml's ignore list. Check both to see the full story.
set -euo pipefail
cd "$(dirname "$0")/.."

# EMPTY ON PURPOSE — every production advisory now fails this job. Keep it that way
# for as long as you can; an entry here is a guard you have switched off.
#
# READ BEFORE ADDING ONE (#178). The match below is `[ "$adv" = "$ok" ]`: **string
# equality on the advisory ID, with NO version awareness whatsoever.** So an entry
# does not suppress "this advisory, in the version we're pinned to" — it suppresses
# **that advisory ID FOREVER, in every version this repo will ever resolve.**
#
# That is not theoretical. The entry removed here was `GHSA-qwww-vcr4-c8h2`
# (react-router RSC-mode CSRF bypass), parked on 2026-08-03 when its 7.x line had no
# patch and npm's only "fix" was a downgrade to 7.11.0. Its own note said *"Retire
# when a patched >=7.18.x lands."* That condition has fired: the advisory's ranges
# are now `>= 7.12.0, < 7.18.2` (first patched **7.18.2**) and `>= 8.0.0, < 8.3.0`
# (first patched **8.3.0**), and this repo resolves react-router / react-router-dom
# at exactly **7.18.2**. Dependabot agrees — its alert for this one is gone.
#
# The reason it had to GO rather than sit here inert: **the same advisory has a
# second, still-unpatched range in 8.x.** #166 proposes upgrading to react-router
# v8. Had that landed on anything in 8.0.0–8.2.x with this entry still present, the
# repo would have re-entered this exact CSRF bypass and this script would have
# reported CLEAN — because the ID matched. The guard most likely to catch a bad v8
# migration would have been disabled by the very note written to track that
# migration. Found by @oldmanbendobot.
#
# So: an entry must carry a retire condition, and **a retire condition that has
# fired is not documentation, it is a live hole.** If you park something here,
# park the smallest thing you can and come back for it.
ALLOWLIST=()

# npm audit exits 1 when advisories exist — that's data, not failure. But an
# infra failure (registry down, bad JSON) must fail LOUD, not read as "clean":
# require a parseable object with a vulnerabilities key before trusting emptiness.
json=$(npm audit --omit=dev --json || true)
if ! jq -e 'type == "object" and has("vulnerabilities")' <<<"$json" >/dev/null 2>&1; then
  echo "npm audit produced no usable JSON — refusing to pass vacuously:"
  echo "$json" | head -20
  exit 1
fi
mapfile -t advisories < <(jq -r '
  [.vulnerabilities[]?.via[]? | objects | .url // empty]
  | unique | .[] | split("/") | last
' <<<"$json")

fail=0
for adv in "${advisories[@]}"; do
  allowed=0
  for ok in "${ALLOWLIST[@]}"; do
    [ "$adv" = "$ok" ] && allowed=1 && break
  done
  if [ "$allowed" = 0 ]; then
    echo "UNALLOWLISTED advisory: $adv"
    fail=1
  fi
done
if [ "$fail" = 1 ]; then
  echo "npm audit found advisories outside the allowlist — see above."
  exit 1
fi
echo "npm audit clean (allowlist: ${#ALLOWLIST[@]} documented entr$( [ ${#ALLOWLIST[@]} = 1 ] && echo y || echo ies ))."
