#!/usr/bin/env bash
# IAM policy-engine proof for the per-lambda dynamo policies (#70).
#
# Feeds every request shape in terraform/iam-request-corpus.json — captured from the real
# Store methods by crates/dynamo/tests/iam_capture.rs — through `aws iam
# simulate-custom-policy` against the SAME policy bytes terraform deploys
# (dynamo-rw-<lambda>.json.tpl), asserting Allow. Then a set of negative probes — the
# blast-radius requests the policies exist to stop — asserting Deny. The simulator is the
# real AWS policy evaluation engine: same verdict authority as a live 403, no infra touched.
#
# Needs: aws cli with iam:SimulateCustomPolicy, jq. Run from anywhere:
#   terraform/policies/simulate.sh
set -euo pipefail
cd "$(dirname "$0")"

TABLE_ARN="arn:aws:dynamodb:us-east-1:123456789012:table/bendobundles"
CORPUS="../iam-request-corpus.json"
pass=0 fail=0

# simulate <lambda> <action> <resource> <expect: allowed|deny> <label> [context-entries...]
simulate() {
  local lambda="$1" action="$2" resource="$3" expect="$4" label="$5"
  shift 5
  local policy decision
  policy="$(sed "s|\${table_arn}|$TABLE_ARN|g" "dynamo-rw-${lambda}.json.tpl")"
  decision="$(aws iam simulate-custom-policy \
    --policy-input-list "$policy" \
    --action-names "$action" \
    --resource-arns "$resource" \
    ${1:+--context-entries "$@"} \
    --query 'EvaluationResults[0].EvalDecision' --output text)"
  local ok
  case "$expect" in
    allowed) [[ "$decision" == "allowed" ]] && ok=1 || ok=0 ;;
    deny) [[ "$decision" == "explicitDeny" || "$decision" == "implicitDeny" ]] && ok=1 || ok=0 ;;
  esac
  if [[ "$ok" == 1 ]]; then
    pass=$((pass + 1))
    printf 'PASS  %-12s %-30s %-8s -> %s  (%s)\n' "$lambda" "$action" "$expect" "$decision" "$label"
  else
    fail=$((fail + 1))
    printf 'FAIL  %-12s %-30s %-8s -> %s  (%s)\n' "$lambda" "$action" "$expect" "$decision" "$label"
  fi
}

# --- positive matrix: every captured request shape must be allowed -----------------------
# tpl files are keyed public/admin/fulfillment; corpus lambdas are the full names.
declare -A TPL=([public-api]=public [admin-api]=admin [fulfillment]=fulfillment)

while IFS=$'\t' read -r lambda method action index leading attrs select retvals; do
  resource="$TABLE_ARN"
  [[ "$index" != "null" ]] && resource="$TABLE_ARN/index/$index"
  ctx=()
  [[ -n "$leading" ]] && ctx+=("ContextKeyName=dynamodb:LeadingKeys,ContextKeyValues=$leading,ContextKeyType=stringList")
  [[ -n "$attrs" ]] && ctx+=("ContextKeyName=dynamodb:Attributes,ContextKeyValues=$attrs,ContextKeyType=stringList")
  [[ "$select" != "null" ]] && ctx+=("ContextKeyName=dynamodb:Select,ContextKeyValues=$select,ContextKeyType=string")
  [[ "$retvals" != "null" ]] && ctx+=("ContextKeyName=dynamodb:ReturnValues,ContextKeyValues=$retvals,ContextKeyType=string")
  simulate "${TPL[$lambda]}" "$action" "$resource" allowed "$method" "${ctx[@]}"
done < <(jq -r '
  to_entries[] as {key: $lambda, value: $methods} |
  $methods | to_entries[] as {key: $method, value: $ops} |
  $ops[] |
  [$lambda, $method, .action, (.index // "null"),
   (.leading_keys | join(",")), (.attributes | join(",")),
   (.select // "null"), (.return_values // "null")] | @tsv' "$CORPUS")

# --- negative probes: the requests these policies exist to make impossible ---------------
LK() { echo "ContextKeyName=dynamodb:LeadingKeys,ContextKeyValues=$1,ContextKeyType=stringList"; }
AT() { echo "ContextKeyName=dynamodb:Attributes,ContextKeyValues=$1,ContextKeyType=stringList"; }

simulate public dynamodb:GetItem "$TABLE_ARN" deny \
  "public reads an admin session" "$(LK 'SESSION#deadbeef')" "$(AT 'pk,sk')"
simulate public dynamodb:PutItem "$TABLE_ARN" deny \
  "public mints an admin session" "$(LK 'SESSION#forged')" "$(AT 'pk,sk,expires_at')"
simulate public dynamodb:Scan "$TABLE_ARN" deny \
  "public scans the table (would bypass every LeadingKeys deny)"
simulate public dynamodb:Scan "$TABLE_ARN/index/listable" deny \
  "public scans the listable GSI"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public sets gift_note (admin's scoped writer)" "$(LK 'LINK#tok')" "$(AT 'pk,sk,gift_note')"
# NOTE the guarantee's honest boundary: revoked/claims_allowed/expires_at ARE in the LINK#
# allowlist because the claim tx names them in its ConditionExpression, and
# dynamodb:Attributes cannot distinguish a condition-read from a SET. The structural wins
# are the names public traffic never utters: gift_note here, and on GAME# the whole
# enforcer family (hidden, owned_by_ben, steam_app_id, ...) — probed next.
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public unhides a game (enforcer attr, not in allowlist)" \
  "$(LK 'GAME#g1')" "$(AT 'pk,sk,hidden,hidden_source')"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public flips owned_by_ben" "$(LK 'GAME#g1')" "$(AT 'pk,sk,owned_by_ben')"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public UpdateItem with ReturnValues=ALL_OLD (full-item read-back)" \
  "$(LK 'GAME#g1')" "$(AT 'pk,sk,body')" \
  "ContextKeyName=dynamodb:ReturnValues,ContextKeyValues=ALL_OLD,ContextKeyType=string"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public updates a session item (explicit deny beats scoped allow)" \
  "$(LK 'SESSION#x')" "$(AT 'pk,sk,body')"
simulate admin dynamodb:GetItem "$TABLE_ARN" deny \
  "admin reads an OIDC nonce" "$(LK 'OIDCSTATE#n')" "$(AT 'pk,sk')"
simulate fulfillment dynamodb:GetItem "$TABLE_ARN" deny \
  "fulfillment reads an admin session" "$(LK 'SESSION#deadbeef')" "$(AT 'pk,sk')"
simulate fulfillment dynamodb:PutItem "$TABLE_ARN" deny \
  "fulfillment mints an admin session" "$(LK 'SESSION#forged')" "$(AT 'pk,sk,expires_at')"
simulate fulfillment dynamodb:DeleteItem "$TABLE_ARN" deny \
  "fulfillment deletes an OIDC nonce" "$(LK 'OIDCSTATE#n')" "$(AT 'pk,sk')"

echo
echo "passed: $pass  failed: $fail"
[[ "$fail" == 0 ]]
