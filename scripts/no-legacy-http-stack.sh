#!/usr/bin/env bash
# Assert the legacy hyper-0.14 / rustls-0.21 HTTP stack stays OUT of Cargo.lock.
#
# WHY THIS EXISTS SEPARATELY FROM `cargo audit`:
#   We landed on the legacy stack because every `aws-sdk-*` crate enables a deprecated
#   `rustls` feature BY DEFAULT (-> aws-smithy-runtime/tls-rustls -> legacy-rustls-ring
#   -> hyper-014 -> h2 0.3). `cargo audit` only complains while an ADVISORY happens to be
#   open against those crates (RUSTSEC-2026-0258 was, from 2026-08-17). If that advisory is
#   ever withdrawn or backported, audit goes quiet and we silently drift back onto an
#   unmaintained TLS/HTTP stack with nothing objecting.
#   `cargo audit` asks "are there known vulns?". This asks "are we on the stack we chose?".
#   Those are different questions and only one of them is stable over time.
#
# HOW TO SATISFY IT: depend on aws-sdk-* with
#   { version = "1", default-features = false, features = ["default-https-client", "rt-tokio"] }
# which keeps the MODERN client (hyper 1.x + rustls 0.23 + aws-lc-rs) and drops only the legacy leg.
#
# rc: 0 = clean, 1 = legacy crate present, 2 = NOT MEASURED (fail closed).
set -uo pipefail

LOCK="${1:-Cargo.lock}"

[ -r "$LOCK" ] || { echo "NOT MEASURED — cannot read $LOCK"; exit 2; }

total=$(grep -c '^name = ' "$LOCK")
# Vacuous-pass guard: an empty/garbled lock would satisfy every "is absent" test below.
if [ "$total" -lt 50 ]; then
  echo "NOT MEASURED — $LOCK lists only ${total} package(s); refusing to report a clean bill from that"
  exit 2
fi

# name<TAB>banned version prefix<TAB>why
BANNED=$'hyper\t0.14.\tlegacy hyper (aws-smithy-http-client feature "hyper-014")\nh2\t0.3.\tEOL h2 line; RUSTSEC-2026-0258 has no 0.3 fix\nrustls\t0.21.\tunmaintained rustls line (pulls rustls-webpki 0.101, sct)\ntokio-rustls\t0.24.\trides the rustls 0.21 chain\nrustls-webpki\t0.101.\trides the rustls 0.21 chain'

fail=0
while IFS=$'\t' read -r crate prefix why; do
  [ -n "$crate" ] || continue
  # versions recorded for this crate, in lock order
  found=$(awk -v c="name = \"$crate\"" '
    $0==c {want=1; next}
    want && /^version = / {gsub(/version = |"/,""); print; want=0}
  ' "$LOCK" | grep "^${prefix}" || true)
  if [ -n "$found" ]; then
    echo "❌ $crate $(echo "$found" | tr '\n' ' ')— $why"
    fail=1
  else
    echo "✅ no $crate ${prefix}x"
  fi
done <<< "$BANNED"

if [ "$fail" -ne 0 ]; then
  echo
  echo "The legacy HTTP/TLS stack is back in $LOCK ($total packages scanned)."
  echo "Almost certainly an aws-sdk-* dependency was added or reverted to default features."
  echo "See the header of $0 for the exact dependency form that keeps it out."
  exit 1
fi

echo "clean — legacy hyper-0.14 / rustls-0.21 stack absent ($total packages scanned)"
exit 0
