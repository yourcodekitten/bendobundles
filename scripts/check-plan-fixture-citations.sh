#!/usr/bin/env bash
# Every fixture-table row in the plan: does <name> actually appear at <file>:<line>?
set -uo pipefail
P=docs/superpowers/plans/2026-09-25-doorstep.md
ok=0; bad=0; skip=0
check(){ # name file line
  local n="$1" f="$2" l="$3"
  if [ ! -r "$f" ]; then echo "  🔴 $n — UNREADABLE $f"; bad=$((bad+1)); return; fi
  if sed -n "${l}p" "$f" | grep -qF "$n"; then
    printf '  ✅ %-28s %s:%s\n' "$n" "$f" "$l"; ok=$((ok+1))
  else
    printf '  🔴 %-28s %s:%s  ACTUAL: %s\n' "$n" "$f" "$l" "$(sed -n "${l}p" "$f" | cut -c1-70)"; bad=$((bad+1))
  fi
}
F=crates/fulfillment/tests/handler_test.rs
A=crates/admin-api/tests/api_test.rs
D=crates/dynamo/src/lib.rs
check store_or_skip        $F 81
check link                 $F 116
check deps                 $F 135
check seed_aged_pending    $F 971
check hours_ago            $F 1007
check body_json            $A 129
check datetime             $A 220
check test_app_with_call_invoker $A 1624
check authed_post          $A 1706
check authed_get           $A 1717
check get_link             $D 1096
check get_claim            $D 1481
check compensate_self_claim $D 2254
check list_pending_claims  $D 2774
check compensate_any       crates/fulfillment/src/lib.rs 2221
check "pub enum FulfillResponse" crates/fulfillment/src/lib.rs 188
check "derive"             crates/fulfillment/src/lib.rs 186
check "ADMIN_REQUEST_HEADER" crates/admin-api/src/lib.rs 178
check "FORBIDDEN"          crates/admin-api/src/lib.rs 208
check route_layer          crates/admin-api/src/lib.rs 144
check ADMIN_CSRF_HEADER    web/src/api.ts 365
check adminSelfClaims      web/src/api.ts 773
check "vi.mock('../api')"  web/src/admin/Ops.test.tsx 8
check renderOps            web/src/admin/Ops.test.tsx 27
check TestLayout           web/src/admin/Ops.test.tsx 17
check mockFetch            web/src/api.test.ts 40
echo "  ── $ok exact · $bad WRONG · denominator $((ok+bad)) cited fixture rows ──"
[ "$bad" -eq 0 ] || exit 1
