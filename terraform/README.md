# bendobundles — Terraform deploy runbook

> **Who deploys this: kitten (the agent) does — every production deploy so far has been kitten's.**
> (Ben may have run the very first bring-up apply; everything since is kitten.) So the operational path is
> **[Deploying as kitten](#deploying-as-kitten-the-agent)** — start there. Kitten doesn't run from this
> repo's directory, so this grep-reachable runbook is the signpost. The Ben-local sections further down
> (prerequisites, one-time bootstrap, local toolchain) are reference + the one-time account setup, not the
> routine loop.

---

## Deploying as kitten (the agent)

**Yes, kitten can deploy — web and terraform both.** The credentials live in `terraform-iam/`: a
`kitten-mgr` IAM user (long-lived key on the box, `sts:AssumeRole`-only) that assumes **`kitten-debug`**
(read-only, the default) or **`kitten-deploy`** (powerful — `terraform apply` + `deploy-web.sh`). Profiles
`kitten-debug` / `kitten-deploy` are in `~/.aws/config`.

> **You MUST pass `--profile kitten-deploy` (or `AWS_PROFILE=kitten-deploy`) for any deploy action.**
> The box's bare instance identity (`…omyac-ombb`) is narrow and **403s** on S3/CloudFront — a plain
> `aws s3 …` will fail confusingly. Use `kitten-deploy` deliberately for deploys, `kitten-debug` for
> read-only inspection.

### Web-only deploy (the common case — no `terraform apply`)

A friend-facing web change (merged PR that only touches `web/`) needs just S3 sync + CloudFront
invalidation. Kitten can do this **fully self-serve — no secrets required.** The box is Node 18 and the
SPA build needs Node 22, so the clean path is **pull the CI-built `web-dist` artifact** (CI builds on
Node 22 on every green push to `main`). (A Node 22 is also installed at `~/.local/node22/bin` if you ever
must build locally — `export PATH="$HOME/.local/node22/bin:$PATH"`, and `rm -rf node_modules && npm ci`
under it first — but the CI artifact avoids all of that.)

```bash
# 1. Grab the CI-built SPA from the latest green run on main (find it: gh run list -R yourcodekitten/bendobundles --branch main)
rm -rf web/dist
gh run download <GREEN_RUN_ID> -R yourcodekitten/bendobundles -n web-dist -D web/dist

# 2. Get the authoritative targets from terraform state (read-only)
cd terraform
AWS_PROFILE=kitten-deploy terraform init -backend-config=backend.hcl -input=false
AWS_PROFILE=kitten-deploy terraform output -raw s3_bucket_id                # brd-prod-ue1-bendobundles-site
AWS_PROFILE=kitten-deploy terraform output -raw cloudfront_distribution_id  # E3M17M9HPGPY0K
cd ..

# 3. DRY-RUN the sync first — a clean SPA republish deletes ONLY the old vite-hashed js/css pair.
#    Anything else showing up as (dryrun) delete → STOP and look.
AWS_PROFILE=kitten-deploy aws s3 sync web/dist s3://<BUCKET> --delete --dryrun

# 4. Real sync + invalidation
AWS_PROFILE=kitten-deploy aws s3 sync web/dist s3://<BUCKET> --delete
AWS_PROFILE=kitten-deploy aws cloudfront create-invalidation --distribution-id <DIST_ID> --paths "/*"

# 5. VERIFY (never report from memory): the live site serves the new bundle
curl -s https://bendobundles.com/ | grep -oE '/assets/index-[A-Za-z0-9_]+\.js'   # matches web/dist/index.html
```

`./deploy-web.sh` wraps steps 2–4 (run it with `AWS_PROFILE=kitten-deploy`, after `web/dist` is in place
and `terraform init` has run). Steps 3 + 5 (dry-run, verify) are kitten's discipline on top of the script.

### Full deploy (infra / lambdas changed — needs `terraform apply`)

Same role and state; the `kitten-deploy` policy is built for exactly this. **Kitten has everything it
needs — no part of this waits on Ben.**

1. **Lambda zips** (only if a Rust crate changed) — pull from the green run and flatten to the names
   Terraform references:
   ```bash
   gh run download <GREEN_RUN_ID> -R yourcodekitten/bendobundles -n lambda-zips -D terraform/artifacts
   for b in public-api admin-api fulfillment; do
     mv terraform/artifacts/$b/bootstrap.zip terraform/artifacts/$b.zip && rmdir terraform/artifacts/$b
   done
   ```
2. **Recreate `production.tfvars`** — gitignored, so kitten writes it fresh each deploy (off-transcript;
   `terraform/production.tfvars` or a scratchpad path). Every value is already on the box:
   - `aws_account_id = "672812236571"` (constant) · `domain_zone_id = "Z05311872JYVFOPFTIVOS"`
     (re-derivable: `AWS_PROFILE=kitten-deploy aws route53 list-hosted-zones`)
   - `humble_username` / `discord_webhook_url` from **`~/.secrets/bendobundles-deploy.env`** (600, outside
     git — the saved deploy secrets; see `code-kitten` `state/decisions.md` 2026-07-06 pointer)
3. **`admin_password_hash` — pull the LIVE value and pass it back verbatim (a no-op). NEVER re-hash the
   password.** The `admin_hash` SSM param (`aws-ssm.tf`) is `value = var.admin_password_hash` with **no
   `ignore_changes`**, so terraform sets it to whatever you pass on every apply. Argon2 with a fresh salt
   produces a *different* PHC string → an in-place UPDATE that **silently resets Ben's live admin login**.
   Feed the current stored value so the change is a no-op:
   ```bash
   export TF_VAR_admin_password_hash="$(AWS_PROFILE=kitten-deploy aws ssm get-parameter \
     --name /brd-prod-ue1-bendobundles-param/admin-hash --with-decryption \
     --query Parameter.Value --output text)"
   # (kitten-deploy can also recover it from state — `terraform state pull` + jq — which works even if the
   #  SSM read grant is ever removed; state ≠ SSM. Either source is fine.)
   ```
   `~/.secrets/…ADMIN_PASSWORD` is the plaintext, kept for break-glass only — do **not** argon2 it for a
   routine apply; re-hashing is the clobber.

```bash
export TF_VAR_discord_webhook_url='…'   # optional; from ~/.secrets, enables cookie-death pings
cd terraform
AWS_PROFILE=kitten-deploy terraform init -backend-config=backend.hcl -input=false
AWS_PROFILE=kitten-deploy terraform plan -var-file=production.tfvars -out=tf.plan
# READ every "will be created/updated/destroyed" line — the count is exact (e.g. 2 crates changed = 2
# updates; a 3rd line, or ANY admin_hash change, is a STOP). Then:
AWS_PROFILE=kitten-deploy terraform apply tf.plan
./deploy-web.sh     # publish the SPA too (AWS_PROFILE=kitten-deploy) if web/ changed
```

**Escalate to Ben** only for genuinely ambiguous blast radius — an unexpected `destroy`, an `admin_hash`
change you didn't intend, a breaking infra change, or unrecognised providers (a wrong-workspace /
foreign-state slip — see [bootstrap step 3](#one-time-bootstrap)). Routine web + code deploys don't need it.

Authoritative outputs (`s3_bucket_id`, `cloudfront_distribution_id`, `site_url`) always come from
`terraform output`; the values inlined in this doc are 2026-07-08 conveniences, not the source of truth.

---

## What this deploys

The Terraform stack provisions the full bendobundles.com production infrastructure in a single AWS
account: a DynamoDB table (product catalogue + key inventory + gift records), three Lambda functions
(public-api, admin-api, fulfillment) behind an API Gateway REST API, an EventBridge schedule for
the daily Humble Bundle sync, SSM parameter store entries for secrets, a CloudFront distribution
fronting an S3 bucket for the SPA, and a Route 53 DNS record. See spec §3 for the architecture
diagram (API → CloudFront → Lambda fanout with `/api/*` and `/admin/api/*` path patterns routed
to the REST API origin; everything else served from S3).

Stack outputs after apply:

| Output | What it is |
|---|---|
| `dynamodb_table_name` | Table name for direct `aws dynamodb` ops |
| `lambda_function_names` | Map of `public_api / admin_api / fulfillment` names |
| `api_stage_invoke_url` | Direct HTTPS invoke URL for the API stage |
| `site_url` | CloudFront distribution domain (also the live site) |
| `s3_bucket_id` | S3 bucket for `./deploy-web.sh` syncs |
| `cloudfront_distribution_id` | Distribution ID for manual invalidations |

---

## Expected cost

Idle cost is ~$0.50/mo (the Route53 zone) — everything else is on-demand /
scale-to-zero and sits inside the AWS free tier at personal scale. The
committed expected-usage model lives in `terraform/infracost-usage.yml`
(~200 friend views + ~20 claims + daily sync per month); CI's `infracost`
job posts the estimate on PRs once the `INFRACOST_API_KEY` repo secret is
set (free key: https://dashboard.infracost.io). Estimates are pre-free-tier,
so treat them as an upper bound.

## Prerequisites

- **Terraform >= 1.10** — required for S3-native state locking (`use_lockfile`)
- **AWS CLI v2** — for `aws dynamodb`, `aws ssm`, and credential setup
- **Rust via `rustup`** (NOT Homebrew) + the arm64 cross-target. Homebrew's Rust has no
  `rustup`, so it cannot cross-compile and cargo-lambda's target auto-add silently no-ops:
  ```bash
  # if `cargo` is currently brew's:  brew uninstall rust && brew install rustup && rustup default stable
  rustup target add aarch64-unknown-linux-gnu
  ```
- **cargo-lambda** — builds the Lambda zips; bundles its own Zig toolchain (`pip3 install cargo-lambda` or `brew install cargo-lambda`)
- **Node 22** — for the SPA build (`./terraform/build.sh` calls `npm run build`)
- **argon2 CLI** — for the one-liner that generates the admin password hash

> **No local toolchain?** CI builds the same artifacts on every push to `main`. Download the
> latest green run's `lambda-zips` into `terraform/artifacts/` and `web-dist` into `web/dist/`,
> then flatten the lambda zips to the names Terraform references (`artifacts/<bin>.zip` — the
> artifact stores them as `<bin>/bootstrap.zip`, which `terraform plan`/`apply` can't see):
>
> ```bash
> gh run download <run-id> -n lambda-zips -D terraform/artifacts
> gh run download <run-id> -n web-dist -D web/dist
> for b in public-api admin-api fulfillment; do
>   mv terraform/artifacts/$b/bootstrap.zip terraform/artifacts/$b.zip && rmdir terraform/artifacts/$b
> done
> ```
>
> That rename is the same thing `build.sh`'s second loop does — with it done, skip `build.sh`.
> (`web-dist` needs no rename: its contents are stored relative to `web/dist/`, exactly where
> `deploy-web.sh` expects them.)

---

## One-time bootstrap

**1. Backend config**

Copy the example and fill in your real state bucket name:

```bash
cp terraform/backend.hcl.example terraform/backend.hcl
# backend.hcl is gitignored — never commit it
$EDITOR terraform/backend.hcl
```

The committed [`backend.hcl.example`](./backend.hcl.example) is the single
source of truth for the values (bucket, key, kms + `encrypt = true`,
`use_lockfile`, `workspace_key_prefix`) — copy it, then adjust the bucket to
your real state bucket.

**2. Generate the admin password hash**

Do this once, store the result in a password manager. Never put the plaintext in tfvars.

```bash
echo -n 'your-chosen-password' | argon2 "$(openssl rand -base64 16)" -id -e
```

The output is a PHC string like `$argon2id$v=19$m=65536,...`. You will pass this as
`TF_VAR_admin_password_hash` (recommended) or in `production.tfvars` (less good — it ends up in
plan output).

**3. Init and workspace** ⚠️ **do not skip the workspace step**

```bash
cd terraform
terraform init -backend-config=backend.hcl
terraform workspace new production   # or `select production` if it already exists
terraform workspace list             # confirm the '*' is on production, NOT default
```

> **Why this is load-bearing, not ceremony:** in the **`default`** workspace the S3 backend
> ignores `workspace_key_prefix` and reads `terraform.tfstate` at the **bucket root** — which is
> very likely a *different* stack's state. Planning against it will propose **destroying that
> other stack**. Always confirm `terraform workspace show` prints `production` before you plan or
> apply. (This bit us once; the only reason nothing was destroyed was an unrelated provider error.)

---

## Deploy loop

Run this sequence any time you want to ship a new version:

```bash
# 1. Build all artifacts (lambda zips → terraform/artifacts/, web bundle → web/dist/)
./terraform/build.sh

# 2. Plan (review the diff before touching live infra)
cd terraform
terraform workspace show          # MUST print 'production' — see bootstrap step 3
terraform plan \
  -var-file=production.tfvars \
  -out=tf.plan

# 3. Apply
terraform apply tf.plan

# 4. Publish the SPA (sync web/dist → S3 + CloudFront invalidation)
./deploy-web.sh
```

> **On the FIRST deploy, the plan must be create-only.** If `terraform plan` shows *any*
> `destroy` (or you see providers you don't recognise, e.g. `google`), stop — you are almost
> certainly in the wrong workspace pointed at foreign state. Re-check step 3.

### First apply: the account-level API Gateway logging role

The stack owns an `aws_api_gateway_account` CloudWatch-logs role (needed because the stage enables
execution logging). This is a **per-region account singleton** — if the account already has one
set from another stack, terraform adopts/overwrites it. Harmless for a single-app account; worth
knowing if you share the account.

### Example `production.tfvars`

Required variables — everything else has a default:

```hcl
aws_account_id = "123456789012"    # your AWS account ID
domain_zone_id = "Z1ABCDEF123456"  # Route53 hosted zone ID for bendobundles.com
```

Keep `admin_password_hash` and `discord_webhook_url` out of the tfvars file. Pass them via
environment variables so they stay out of plan output and git history:

```bash
export TF_VAR_admin_password_hash='$argon2id$v=19$...'   # the PHC string from bootstrap step 2
export TF_VAR_discord_webhook_url='https://discord.com/api/webhooks/...'  # optional
```

### Optional variables (all have defaults)

| Variable | Default | Notes |
|---|---|---|
| `region` | `us-east-1` | CloudFront ACM requires us-east-1; don't change |
| `namespace` | `brd` | Org namespace for resource labels |
| `role` | `production` | Context role tag |
| `domain_zone_name` | `bendobundles.com` | Route53 zone name |
| `route53_profile` | `null` | AWS profile for the Route53 account if different from the main account |
| `sync_schedule_expression` | `cron(0 9 * * ? *)` | EventBridge schedule for daily Humble sync (09:00 UTC = pre-dawn US-East) |
| `discord_webhook_url` | `null` | Omit entirely to disable cookie-death pings |
| `humble_username` | `null` | Enables self-login: creates the `humble-password` / `humble-totp-secret` param containers (values set out of band) so fulfillment logs in and maintains its own session |

---

## After first deploy

**1. Seed the humble session**

With `humble_username` set, put the real account password and TOTP seed into the
`humble-password` / `humble-totp-secret` SSM params (they're created at `UNSET`; overwrite them
via the AWS console/CLI — never through terraform, so they stay out of state). The fulfillment
Lambda then logs in on its own and persists the session to the `humble-cookie` param.

Break-glass (or if self-login is off): write a browser-captured `_simpleauth_sess` cookie value
into the `humble-cookie` param directly (`aws ssm put-parameter --overwrite --type SecureString`).
`--overwrite` preserves the container's Advanced tier that terraform sets; only if you ever
delete-and-recreate the param do you also need `--tier Advanced` (a self-login session can exceed
the 4 KB Standard cap). This session is what the fulfillment Lambda uses to fetch key inventory.

**2. Trigger a manual sync**

Hit the "sync now" button in the admin panel (or invoke the fulfillment Lambda directly) to
populate the key inventory from Humble. Confirm the DynamoDB table has rows afterwards:

```bash
aws dynamodb scan \
  --table-name "$(terraform output -raw dynamodb_table_name)" \
  --select COUNT
```

**3. Run the VERIFY checklist at first real gifting** (see below).

---

## VERIFY at first real gifting

Run through this checklist on the first live gift order — not in a staging env, on a real
multi-key Humble order with a real recipient:

- [ ] **Key-index handling on a real multi-key order** — fulfillment correctly selects an unspent
  key from the inventory; the keyindex advances; no double-assign on retry.
- [ ] **RedeemRefused / AmbiguousRedeem park behavior** — if Humble rejects the redeem or returns
  an ambiguous result, the gift record parks in the correct state (not silently dropped or marked
  redeemed); admin panel shows the parked order for manual intervention.
- [ ] **Gift URL renders and copies on a real phone** — open the gift link on an actual mobile
  device; the landing page renders; the copy-to-clipboard button works.
- [ ] **Cookie-death Discord ping fires** — let the Humble session expire (or force it by
  overwriting the `humble-cookie` param with garbage) with `discord_webhook_url` set; confirm the
  self-heal/dead-session ping arrives in the configured channel.

---

## Module pin note

`module "site"` consumes the registry release:

```hcl
source  = "bendoerr-terraform-modules/cloudfront-and-s3-origin/aws"
version = "0.5.0"
```

`v0.5.0` ships the additional-origins support from
[cf-s3-origin#137](https://github.com/bendoerr-terraform-modules/terraform-aws-cloudfront-and-s3-origin/pull/137).
To take a future release: bump `version`, run `terraform init -upgrade`, replan.
