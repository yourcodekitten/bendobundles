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

# simulate <lambda> <action> <resource> <expect: allowed|deny> <label> [context-json]
# context is a JSON array of ContextEntry objects — never CLI shorthand, whose comma
# parsing is ambiguous for multi-valued stringList keys.
simulate() {
  local lambda="$1" action="$2" resource="$3" expect="$4" label="$5" ctx="${6:-[]}"
  local policy decision
  policy="$(sed "s|\${table_arn}|$TABLE_ARN|g" "dynamo-rw-${lambda}.json.tpl")"
  decision="$(aws iam simulate-custom-policy \
    --policy-input-list "$policy" \
    --action-names "$action" \
    --resource-arns "$resource" \
    --context-entries "$ctx" \
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

while IFS=$'\t' read -r lambda method action index ctx; do
  resource="$TABLE_ARN"
  [[ "$index" != "null" ]] && resource="$TABLE_ARN/index/$index"
  simulate "${TPL[$lambda]}" "$action" "$resource" allowed "$method" "$ctx"
done < <(jq -r '
  def ctx: [
    (if (.leading_keys | length) > 0 then
      {ContextKeyName: "dynamodb:LeadingKeys", ContextKeyValues: .leading_keys, ContextKeyType: "stringList"} else empty end),
    (if (.attributes | length) > 0 then
      {ContextKeyName: "dynamodb:Attributes", ContextKeyValues: .attributes, ContextKeyType: "stringList"} else empty end),
    (if .select then
      {ContextKeyName: "dynamodb:Select", ContextKeyValues: [.select], ContextKeyType: "string"} else empty end),
    (if .return_values then
      {ContextKeyName: "dynamodb:ReturnValues", ContextKeyValues: [.return_values], ContextKeyType: "string"} else empty end)
  ];
  to_entries[] as {key: $lambda, value: $methods} |
  $methods | to_entries[] as {key: $method, value: $ops} |
  $ops[] |
  [$lambda, $method, .action, (.index // "null"), (ctx | tojson)] | @tsv' "$CORPUS")

# --- negative probes: the requests these policies exist to make impossible ---------------
# ctx <LeadingKeys or ""> <comma-joined Attributes or ""> [ReturnValues] -> JSON array
ctx() {
  jq -cn --arg lk "${1:-}" --arg at "${2:-}" --arg rv "${3:-}" '[
    (if $lk != "" then {ContextKeyName:"dynamodb:LeadingKeys",ContextKeyValues:[$lk],ContextKeyType:"stringList"} else empty end),
    (if $at != "" then {ContextKeyName:"dynamodb:Attributes",ContextKeyValues:($at|split(",")),ContextKeyType:"stringList"} else empty end),
    (if $rv != "" then {ContextKeyName:"dynamodb:ReturnValues",ContextKeyValues:[$rv],ContextKeyType:"string"} else empty end)
  ]'
}

simulate public dynamodb:GetItem "$TABLE_ARN" deny \
  "public reads an admin session" "$(ctx 'SESSION#deadbeef' 'pk,sk')"
simulate public dynamodb:PutItem "$TABLE_ARN" deny \
  "public mints an admin session" "$(ctx 'SESSION#forged' 'pk,sk,expires_at')"
simulate public dynamodb:Scan "$TABLE_ARN" deny \
  "public scans the table (would bypass every LeadingKeys deny)"
simulate public dynamodb:Scan "$TABLE_ARN/index/listable" deny \
  "public scans the listable GSI"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public sets gift_note (admin's scoped writer)" "$(ctx 'LINK#tok' 'pk,sk,gift_note')"
# NOTE the guarantee's honest boundary: revoked/claims_allowed/expires_at ARE in the LINK#
# allowlist because the claim tx names them in its ConditionExpression, and
# dynamodb:Attributes cannot distinguish a condition-read from a SET. The structural wins
# are the names public traffic never utters: gift_note here, and on GAME# the whole
# enforcer family (hidden, owned_by_ben, steam_app_id, ...) — probed next.
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public unhides a game (enforcer attr, not in allowlist)" \
  "$(ctx 'GAME#g1' 'pk,sk,hidden,hidden_source')"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public flips owned_by_ben" "$(ctx 'GAME#g1' 'pk,sk,owned_by_ben')"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public UpdateItem with ReturnValues=ALL_OLD (full-item read-back)" \
  "$(ctx 'GAME#g1' 'pk,sk,body' ALL_OLD)"
simulate public dynamodb:UpdateItem "$TABLE_ARN" deny \
  "public updates a session item (explicit deny beats scoped allow)" \
  "$(ctx 'SESSION#x' 'pk,sk,body')"
simulate admin dynamodb:GetItem "$TABLE_ARN" deny \
  "admin reads an OIDC nonce" "$(ctx 'OIDCSTATE#n' 'pk,sk')"
simulate fulfillment dynamodb:GetItem "$TABLE_ARN" deny \
  "fulfillment reads an admin session" "$(ctx 'SESSION#deadbeef' 'pk,sk')"
simulate fulfillment dynamodb:PutItem "$TABLE_ARN" deny \
  "fulfillment mints an admin session" "$(ctx 'SESSION#forged' 'pk,sk,expires_at')"
simulate fulfillment dynamodb:DeleteItem "$TABLE_ARN" deny \
  "fulfillment deletes an OIDC nonce" "$(ctx 'OIDCSTATE#n' 'pk,sk')"

echo
echo "passed: $pass  failed: $fail"
[[ "$fail" == 0 ]]
