# Runbook: #158 Out-of-Band Redemption Deploy

## Overview
Deploy sequence for #158 (out-of-band redemption). Follow each step in order. A count is a measurement, not a bound — the pre-press gate must show exactly `1/0/1` before proceeding.

## Pre-Deploy
- Merge the PR
- All CI checks must be green

## Deploy Steps

### Step 1: Terraform Deploy
Run terraform deploy per `terraform/README.md`.

**Required parameters:**
- `boundary_arn` — supply verbatim (no modifications)
- `ops_alarm_email` — mandatory
- `admin_hash` — supply verbatim as given

Example:
```bash
cd terraform/
terraform apply -var="boundary_arn=<value>" -var="ops_alarm_email=<value>" -var="admin_hash=<value>"
```

### Step 2: Pre-Press Gate (Changelist Verification)
Run the deploy-time re-enumeration gate **immediately before the first post-deploy sync**:

```bash
scripts/158-deploy-changelist.sh
```

**Expected output:**
```
CHANGELIST: reroute=1 armA=0 audit=1
GATE: 1/0/1 — matches the signed changelist. Clear to press.
```

**If gate fails or shows anything other than `1/0/1`:**
1. Script will print the full changelist
2. **STOP deployment immediately**
3. Take the printed list and the gate output to Ben before any sync fires
4. Do NOT proceed to Step 3 until gate passes with `1/0/1`

**Gate exit codes:**
- Exit 0: `1/0/1` match — safe to proceed
- Exit 1: Any other count, or any order fetch failure (INCONCLUSIVE) — do not proceed

### Step 3: Trigger First Sync
Once gate passes with `1/0/1`, trigger the first post-deploy sync and await completion.

Monitor sync progress:
- Check logs for claim processing
- Watch for redemption audit operations
- Confirm no errors in sync worker output

### Step 4: Verification
After sync completes, verify all three changes:

1. **Claim Status**: Claim `3f46c058` must show `Fulfilled`
   - Check in dashboard or via API
   - Confirm redemption status reflects post-deploy state

2. **Row De-listing**: Row `GAME#HAXSVMZHBvK2E7dW:mylittleuniverse_row_choice_steam` must be removed from LISTABLE
   - Query DynamoDB to confirm row is no longer queryable via listable GSI
   - Verify it is not accessible to public browse/listing

3. **Discord Notifications**: Confirm both completion pings in Discord
   - Sync completion ping in ops channel
   - Audit completion ping with final changelist metrics

### Step 5: Ship Green
Once all three verifications pass:
- Mark deploy as complete
- Notify Ben if any anomalies were detected (unexpected counts, retries, timing)
- Close the #158 issue

## Troubleshooting

### Gate fails with `INCONCLUSIVE`
- One or more order fetches from Humble Bundle failed
- Network issue or API temporary unavailability
- Wait a moment, then re-run `scripts/158-deploy-changelist.sh`
- Do NOT proceed if fetch failures persist

### Gate fails with count mismatch (e.g., `2/0/1` instead of `1/0/1`)
- The changelist does not match the signed deployment record
- Print the full output and take it to Ben
- Do not proceed until count matches exactly

### Sync hangs or errors
- Check sync worker logs
- Do not mark as complete until sync finishes cleanly
- If unrecoverable, escalate to Ben with logs

## Contacts
- **Deploy Lead**: Ben (on Discord)
- **Ops Support**: OldManBendoBot (if infrastructure questions)
