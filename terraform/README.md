# bendobundles — Terraform deploy runbook

> **This is Ben's complete deploy guide.** Read it top to bottom the first time; afterwards you'll
> only need the [Deploy loop](#deploy-loop) section.

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
- **cargo-lambda** — builds the Lambda zips; bundles its own Zig toolchain (`pip3 install cargo-lambda` or `brew install cargo-lambda`)
- **Node 22** — for the SPA build (`./terraform/build.sh` calls `npm run build`)
- **argon2 CLI** — for the one-liner that generates the admin password hash

---

## One-time bootstrap

**1. Backend config**

Copy the example and fill in your real state bucket name:

```bash
cp terraform/backend.hcl.example terraform/backend.hcl
# backend.hcl is gitignored — never commit it
$EDITOR terraform/backend.hcl
```

The example values:

```hcl
bucket               = "bd-prod-ue1-tfstate-store" # confirm your real state bucket
key                  = "terraform.tfstate"
kms_key_id           = "alias/aws/s3"
region               = "us-east-1"
use_lockfile         = true  # S3-native locking (TF >= 1.10); no dynamodb_table needed
workspace_key_prefix = "bendobundles"
```

**2. Generate the admin password hash**

Do this once, store the result in a password manager. Never put the plaintext in tfvars.

```bash
echo -n 'your-chosen-password' | argon2 "$(openssl rand -base64 16)" -id -e
```

The output is a PHC string like `$argon2id$v=19$m=65536,...`. You will pass this as
`TF_VAR_admin_password_hash` (recommended) or in `production.tfvars` (less good — it ends up in
plan output).

**3. Init and workspace**

```bash
cd terraform
terraform init -backend-config=backend.hcl
terraform workspace new production   # or `select production` if it already exists
terraform workspace list             # confirm you're in production
```

---

## Deploy loop

Run this sequence any time you want to ship a new version:

```bash
# 1. Build all artifacts (lambda zips → terraform/artifacts/, web bundle → web/dist/)
./terraform/build.sh

# 2. Plan (review the diff before touching live infra)
cd terraform
terraform plan \
  -var-file=production.tfvars \
  -out=tf.plan

# 3. Apply
terraform apply tf.plan

# 4. Publish the SPA (sync web/dist → S3 + CloudFront invalidation)
./deploy-web.sh
```

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
| `namespace` | `bd` | Org namespace for resource labels |
| `role` | `production` | Context role tag |
| `domain_zone_name` | `bendobundles.com` | Route53 zone name |
| `route53_profile` | `null` | AWS profile for the Route53 account if different from the main account |
| `sync_schedule_expression` | `cron(0 9 * * ? *)` | EventBridge schedule for daily Humble sync (09:00 UTC = pre-dawn US-East) |
| `discord_webhook_url` | `null` | Omit entirely to disable cookie-death pings |

---

## After first deploy

**1. Paste the humble cookie**

Log into `/admin` with the password you hashed in bootstrap. Under Ops, paste the Humble Bundle
session cookie. This is what the fulfillment Lambda uses to fetch key inventory.

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
- [ ] **Cookie-death Discord ping fires** — let the Humble cookie expire (or force it via the
  admin panel) with `discord_webhook_url` set; confirm the ping arrives in the configured channel.

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
