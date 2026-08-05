# Runbook: #158 Out-of-Band Redemption Deploy

## Overview
Deploy sequence for #158 (out-of-band redemption). Follow each step in order. A count is a measurement, not a bound — the pre-press gate must show exactly `1/0/1` before proceeding.

## Pre-Deploy
- Merge the PR
- All CI checks must be green

## Deploy Steps

### Step 1: Terraform Deploy
Run terraform deploy per `terraform/README.md` ("Deploying as kitten" → "Full deploy"). `boundary_arn`
and `admin_hash` are not real variable names — the real ones are `lambda_permissions_boundary_arn` and
`admin_password_hash`, and both are already carried in `production.tfvars` (the boundary ARN is a
constant; the admin hash is the live value pulled verbatim — see the README, never re-hash it).
`ops_alarm_email` is also in `production.tfvars`. Run from repo root:

```bash
git checkout main && git pull        # post-merge
cd terraform
AWS_PROFILE=kitten-deploy terraform plan -var-file=production.tfvars -out=tf.plan
# READ every resource line of the plan. Expected: ONLY the lambda code/hash updates.
# ANY admin_password_hash line, ANY IAM/boundary line, ANY destroy — STOP, take the
# plan output to ben before touching apply. NEVER pass admin_password_hash by hand;
# never re-hash it — production.tfvars carries the verbatim stored hash.
AWS_PROFILE=kitten-deploy terraform apply tf.plan
cd ..
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
Once gate passes with `1/0/1`, trigger the first post-deploy sync from repo root (still at repo root
after Step 2 — no `cd` needed) and await completion.

Trigger it one of two ways:
- **Admin UI**: log in, hit the "sync now" button on the Ops page (`web/src/admin/Ops.tsx`).
- **API directly**: `POST /admin/api/sync` with an authenticated admin session (route registered in
  `crates/admin-api/src/lib.rs`; fire-and-forget — a full backfill runs in the background).

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
   - Grep for `"reconcile recovered the already-revealed key for self claim"` (claim-recovery ping, carries claim id 3f46c058)
   - Grep for `"shelf audit: pulled"` (audit de-list ping, carries "My Little Universe")

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
