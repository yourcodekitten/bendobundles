# terraform-iam — kitten's access to bendobundles

Creates the identities kitten uses to debug (and, when asked, deploy) the
bendobundles stack. Separate state from the main stack on purpose.

## Shape

- **`kitten-mgr`** — an IAM user whose long-lived access key is handed to
  kitten. It has **no permissions of its own** except `sts:AssumeRole` on the
  three roles below. A leaked key can only assume a role; every assumed action is
  short-lived and attributable in CloudTrail.
- **`kitten-debug`** (read-only) — tail logs, read Lambda config, inspect
  DynamoDB item/claim state, look at API Gateway / CloudFront / S3 / EventBridge.
  **Explicitly denied** `ssm:GetParameter*` and `kms:Decrypt` — kitten can never
  read the humble session cookie or admin hash, even holding these credentials.
- **`kitten-maintenance`** (item data-plane) — run-once operator bins
  (`backfill_details`): Get/BatchGet/Query/Scan/Put/UpdateItem on the app
  tables + indexes, **no DeleteItem**, nothing else. Same
  `ssm:GetParameter*`/`kms:Decrypt` hard-deny as debug. Exists so backfills
  never ride console hand-edits again (#59, #71).
- **`kitten-deploy`** (powerful) — enough to `terraform apply` the main stack
  and run `deploy-web.sh`. Assumed deliberately, only for deploys. Its terraform
  state access is scoped to the **`bendobundles`** prefix only — **not** this IAM
  stack — so it can deploy the app but cannot rewrite its own permissions.

## Apply (ben, once)

```sh
cd terraform-iam
cp backend.hcl.example backend.hcl        # confirm bucket
terraform init -backend-config=backend.hcl
terraform plan  -var aws_account_id=672812236571 -out tf.plan
# review the IAM in the plan, especially the kitten-deploy policy
terraform apply tf.plan
```

## Hand kitten the credentials

```sh
terraform output kitten_manager_access_key_id
terraform output -raw kitten_manager_secret_access_key   # sensitive
terraform output kitten_debug_role_arn
terraform output kitten_deploy_role_arn
terraform output kitten_maintenance_role_arn
```

Give kitten the access key id + secret + both role ARNs. Kitten configures a
profile that assumes the debug role by default, and the deploy role only when
deploying.

## Review notes (the deploy role is the one to eyeball)

- `kitten-deploy` is close to admin-over-these-services by nature ("can run
  terraform" ≈ "can change the stack"). It's scoped by service and, where the
  service allows, by resource (`brd-prod-ue1-bendobundles*`, the site bucket,
  the state prefix, the app's IAM roles). The **broadest** grants — flagged in
  the policy comments — are `apigateway` / `cloudfront` / `acm` / `kms`, which
  don't scope cleanly by resource at create time.
- The deploy role **can** decrypt the app SSM secrets (terraform manages those
  params and reads their values on refresh). That's inherent to owning them,
  and it's exactly why routine debugging uses `kitten-debug`, which cannot.
- The manager user's secret lands in this stack's (encrypted) state. To avoid
  that, swap `aws_iam_access_key` to a `pgp_key`-encrypted key and decrypt it
  yourself — say the word and I'll switch it.

## Notes

- `.gitignore` covers `terraform-iam/.terraform/`, `backend.hcl`, `*.tfplan`.
- This stack is applied with **ben's** admin creds — kitten never applies its
  own access.
