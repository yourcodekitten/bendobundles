#!/usr/bin/env bash
# 158-deploy-changelist.sh — deploy-time re-enumeration gate for #158 (read-only).
# Prints the three-clause changelist and exits 0 IFF it is exactly reroute=1 armA=0 audit=1.
# ANY order fetch failure => INCONCLUSIVE => exit 1 (an empty terminal is not a passing check).
# A count is a measurement, not a bound: run this immediately before the first post-deploy
# sync; on any non-1/0/1 result STOP and take the printed list to ben.
set -euo pipefail
PROFILE_DB="${PROFILE_DB:-kitten-debug}"
PROFILE_SSM="${PROFILE_SSM:-kitten-deploy}"
T=brd-prod-ue1-bendobundles-table
R=us-east-1
SSM_PARAM=/brd-prod-ue1-bendobundles-param/humble-cookie

echo "== clause (a)+(b): pending claims vs their game rows =="
CLAIMS=$(aws dynamodb query --profile "$PROFILE_DB" --region "$R" --table-name "$T" \
  --index-name pending-claims --key-condition-expression "gsi2pk = :p" \
  --expression-attribute-values '{":p":{"S":"PENDINGCLAIM"}}' --output json \
  | jq -c '[.Items[].body.S | fromjson]')
REROUTE=0; ARMA=0
while read -r c; do
  gid=$(echo "$c" | jq -r '.game_id')
  has_snap=$(echo "$c" | jq 'has("choice_pre_tpks") and (.choice_pre_tpks != null)')
  rc=$(aws dynamodb get-item --profile "$PROFILE_DB" --region "$R" --table-name "$T" \
    --key "{\"pk\":{\"S\":\"GAME#$gid\"},\"sk\":{\"S\":\"META\"}}" --output json \
    | jq '.Item.body.S | if . then (fromjson | .requires_choice) else null end')
  if [ "$has_snap" = "true" ] && [ "$rc" = "false" ]; then
    REROUTE=$((REROUTE+1))
    echo "REROUTE: $(echo "$c" | jq -c '{id, game_id, self:(.link_token=="SELF")}')"
  fi
  if [ "$has_snap" = "false" ] && [ "$rc" = "true" ]; then
    ARMA=$((ARMA+1))
    echo "ARM-A: $(echo "$c" | jq -c '{id, game_id, self:(.link_token=="SELF")}')"
  fi
done < <(echo "$CLAIMS" | jq -c '.[]')

echo "== clause (c): LISTABLE rows vs live order truth =="
ROWS=$(aws dynamodb query --profile "$PROFILE_DB" --region "$R" --table-name "$T" \
  --index-name listable --key-condition-expression "gsi1pk = :p" \
  --expression-attribute-values '{":p":{"S":"LISTABLE"}}' --output json \
  | jq -c '[.Items[].body.S | fromjson | {id, gamekey, machine_name, title}]')
COOKIE=$(aws ssm get-parameter --profile "$PROFILE_SSM" --region "$R" \
  --name "$SSM_PARAM" --with-decryption --query Parameter.Value --output text)
PULLS=0; CHECKED=0; FETCH_FAIL=0
for gk in $(echo "$ROWS" | jq -r '[.[].gamekey] | unique | .[]'); do
  ORDER=$(printf 'header = "Cookie: _simpleauth_sess=%s"\n' "$COOKIE" \
    | curl -sfS -K - "https://www.humblebundle.com/api/v1/order/$gk?all_tpkds=true" || echo '{}')
  if [ "$(echo "$ORDER" | jq 'has("tpkd_dict")')" != "true" ]; then
    FETCH_FAIL=$((FETCH_FAIL+1)); echo "FETCH-FAIL: $gk"; sleep 0.3; continue
  fi
  while read -r row; do
    mn=$(echo "$row" | jq -r '.machine_name')
    CHECKED=$((CHECKED+1))
    verdict=$(echo "$ORDER" | jq -c --arg mn "$mn" \
      '[.tpkd_dict.all_tpks[]? | select(.machine_name==$mn)]
       | if length==0 then {absent:true}
         else (.[0] | {revealed:(.redeemed_key_val != null), expired:.is_expired}) end')
    if [ "$(echo "$verdict" | jq '.absent // false')" = "true" ]; then
      continue
    fi
    if [ "$(echo "$verdict" | jq '.revealed or .expired')" = "true" ]; then
      PULLS=$((PULLS+1))
      echo "AUDIT-WOULD-PULL: $(echo "$row" | jq -c '{id,title}') $verdict"
    fi
  done < <(echo "$ROWS" | jq -c --arg gk "$gk" '.[] | select(.gamekey==$gk)')
  sleep 0.3
done

echo "rows_checked=$CHECKED order_fetch_fail=$FETCH_FAIL"
if [ "$FETCH_FAIL" -gt 0 ]; then
  echo "INCONCLUSIVE: $FETCH_FAIL order(s) unfetched — their rows are UNVERIFIED. Do not press."
  exit 1
fi
echo "CHANGELIST: reroute=$REROUTE armA=$ARMA audit=$PULLS"
if [ "$REROUTE" -eq 1 ] && [ "$ARMA" -eq 0 ] && [ "$PULLS" -eq 1 ]; then
  echo "GATE: 1/0/1 — matches the signed changelist. Clear to press."
  exit 0
else
  echo "GATE: MISMATCH vs signed 1/0/1 — STOP. Take the list above to ben before anything runs."
  exit 1
fi
