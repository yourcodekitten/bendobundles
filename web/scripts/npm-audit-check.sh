#!/usr/bin/env bash
# npm audit with a dated allowlist (#81) — npm has no per-advisory ignore, and the
# only "fix" it offers for the entry below is a DOWNGRADE. This fails on any
# production-dependency advisory NOT explicitly allowlisted here, so new ones
# still break CI while documented-unreachable ones don't bury the signal.
set -euo pipefail
cd "$(dirname "$0")/.."

# GHSA-qwww-vcr4-c8h2 — react-router RSC-mode CSRF bypass (affects 7.12.0–8.2.0,
# no patched 7.x as of 2026-08-03; npm's fix is a downgrade to 7.11.0). This app
# is a plain Vite SPA: no RSC, no data-router actions — the vulnerable path is
# unreachable (#81 audit, re-verified). Retire when a patched >=7.18.x lands.
ALLOWLIST=(
  "GHSA-qwww-vcr4-c8h2"
)

json=$(npm audit --omit=dev --json || true)
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
