# Plan 4: Terraform / Deploy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `terraform/` stack that deploys bendobundles (3 lambdas, API Gateway, DynamoDB, SSM, EventBridge, S3+CloudFront SPA) from `bendoerr-terraform-modules/*` legos, plus the CI artifact builds and the one module enhancement the legos are missing.

**Architecture:** One CloudFront distribution serves everything on one origin domain: the SPA from S3 (default behavior, SPA 403/404→index.html) and `/api/*` + `/admin/api/*` routed to a single REGIONAL API Gateway REST API fronting public-api and admin-api. Same-origin is the deliberate choice: every SPA fetch is a relative path and `adminLogin` uses `credentials: 'same-origin'`. The considered alternative — `api.bendobundles.com` as a separate origin — is *workable* (a subdomain is same-SITE, so the `SameSite=Strict` session cookie itself survives) but costs CORS-with-credentials middleware in both API crates (preflight handling, exact-origin echo — wildcards are forbidden with credentials), an API-base-URL config in the web bundle, `credentials: 'include'` everywhere, preflight latency on every claim POST, and the classic credentialed-CORS security surface — versus **zero application code changes** for CloudFront path routing. The `cloudfront-and-s3-origin` module only supports its single S3 origin today, so Task 1 is an enhancement PR to that module (additional origins + ordered cache behaviors + apex-domain support); the stack consumes the enhanced version.

**Tech Stack:** Terraform ≥1.10 (S3-native state locking), AWS provider `~> 6.0`, bendoerr-terraform-modules (context v0.5.2, label v1.0.1, lambda v0.3.0, apigateway v1.1.1, cloudfront-and-s3-origin v0.5.0-after-Task-1), cargo-lambda (arm64, `provided.al2023`), raw `aws_*` resources for DynamoDB/SSM/EventBridge (no org lego exists for those — confirmed by org survey).

## Ben decisions (defaults chosen; flag at PR time, all apply-time-only)

1. **Apex domain**: stack defaults to serving `bendobundles.com` itself (`use_apex_domain = true`, added to the module in Task 1). Alternative is a label-derived subdomain.
2. **State backend**: partial `backend "s3" {}` + committed `backend.hcl.example` (bucket `bd-prod-ue1-tfstate-store`, `use_lockfile = true`, `workspace_key_prefix = "bendobundles"`) — ben confirms/edits his real bucket at init. His creds, his apply (spec §12).
3. **AWS account id**: `aws_account_id` variable, no default, `allowed_account_ids` guard.

## Global Constraints

- Deployment v1 (spec §12): **CI builds artifacts; ben runs `terraform apply` locally.** Nothing in this plan gives CI AWS credentials.
- Trust boundary (spec §3): public-api has **zero** SSM access; only fulfillment + admin-api touch the humble-cookie param (fulfillment: Get; admin-api: Get+Put — its paste/rollback flow reads the old value by design, shipped in plan 2); admin-hash param readable by admin-api only.
- Runtime contract (extracted from `crates/*/src/main.rs`, exact): public-api needs `TABLE_NAME`, `FULFILLMENT_FN`; admin-api needs `TABLE_NAME`, `FULFILLMENT_FN`, `ADMIN_HASH_PARAM`, `HUMBLE_COOKIE_PARAM`; fulfillment needs `TABLE_NAME`, `HUMBLE_COOKIE_PARAM`, optional `DISCORD_WEBHOOK_PARAM`. All panic-if-unset except the webhook. `HUMBLE_COOKIE_PARAM` must be the SAME param name for admin-api and fulfillment.
- Dynamo schema must mirror `dynamo::Store::create_table_for_tests` exactly: PAY_PER_REQUEST; `pk`/`sk` (S) primary; GSI `listable` = `gsi1pk`/`gsi1sk` (S, ALL); GSI `pending-claims` = `gsi2pk`/`gsi2sk` (S, ALL); **TTL on attribute `ttl`** (sessions; schema.rs says "terraform will enable it in plan 4" — this is that).
- EventBridge daily sync needs **no input transformer**: fulfillment routes any payload with `"source": "aws.events"` to `FulfillRequest::Sync` (main.rs:52-63).
- lambdas: `provided.al2023`, `arm64`, handler `"bootstrap"`, zips from `cargo lambda build --release --arm64 --output-format zip` → `target/lambda/<bin>/bootstrap.zip`.
- Node **22** pinned wherever web builds happen (react-router 7 + vite floor; plan-3 carry).
- All commits GPG-signed (`-S`), authored `code kitten <yourcodekitten@gmail.com>`.
- Org module repos (Task 1) gate on: terraform fmt, terraform-docs regen + prettier, tflint, terratest, CodeRabbit. Regenerate docs + run prettier after any variables/outputs change or CI goes red (lesson from tfstate#203).
- CF module context wiring is `context = module.context.shared` (NOT `module.context` — the apigateway example's bare `module.context` works only by structural typing; use `.shared` everywhere for consistency with lambda/cf-s3 examples).

## File Structure (stack — `yourcodekitten/bendobundles`)

```
terraform/
  tf-versions.tf        # required_version >= 1.10, aws ~> 6.0, configuration_aliases
  tf-backend.tf         # partial backend "s3" {}
  backend.hcl.example   # documented backend values for ben
  tf-provider.tf        # aws + aws.route53 alias, allowed_account_ids guard
  tf-variables.tf       # all stack inputs
  tf-outputs.tf         # site URL, api id, bucket, distribution id, table name, fn names
  main.tf               # context module (the only module in main.tf)
  aws-dynamodb.tf       # the single table (raw resource + label)
  aws-ssm.tf            # 3 params (raw resources + label)
  aws-lambda.tf         # 3 × terraform-aws-lambda + inline IAM
  aws-apigateway.tf     # apigateway module (OpenAPI body) + 2 lambda permissions
  aws-eventbridge.tf    # daily sync rule + target + permission
  aws-cloudfront.tf     # cloudfront-and-s3-origin (enhanced) — SPA + API behaviors
  build.sh              # cargo lambda build ×3 + web build → artifacts
  deploy-web.sh         # s3 sync web/dist + cloudfront invalidation (ben, post-apply)
  README.md             # runbook: bootstrap SSM values, init, apply, deploy-web, VERIFY list
.github/workflows/ci.yml   # + terraform job, + artifacts job
```

Module enhancement (Task 1) lives in `bendoerr-terraform-modules/terraform-aws-cloudfront-and-s3-origin` on branch `kitten/additional-origins`.

---

### Task 1: Module enhancement PR — additional origins, ordered cache behaviors, apex domain

**Repo:** `bendoerr-terraform-modules/terraform-aws-cloudfront-and-s3-origin` (clone fresh; branch `kitten/additional-origins`). This is OMBB's org: follow its gates (fmt, terraform-docs, prettier, tflint, terratest, CodeRabbit). I authored #114/#116 there; same flow.

**Files:**
- Modify: `variables.tf` (3 new vars)
- Modify: `aws-cloudfront.tf` (dynamic `origin` + `ordered_cache_behavior` blocks; apex alias local)
- Modify: `README.md` (terraform-docs regen + prettier)
- Create: `examples/api-behaviors/` (ctx.tf, main.tf, variables.tf — mirrors examples/simple shape)

**Interfaces:**
- Produces (consumed by Task 7): module vars `additional_origins`, `ordered_cache_behaviors`, `use_apex_domain` with EXACTLY the shapes below; existing outputs unchanged (`s3_bucket_id`, `cloudfront_distribution_id`, `cloudfront_distribution_alias_domain_name`, ...).

- [ ] **Step 1: Branch + new variables**

Append to `variables.tf`:

```hcl
variable "use_apex_domain" {
  type        = bool
  default     = false
  nullable    = false
  description = "Serve the zone apex (var.domain_zone_name itself) as the distribution's primary alias instead of the label-derived subdomain."
}

variable "additional_origins" {
  type = list(object({
    origin_id   = string
    domain_name = string
    origin_path = optional(string, "")
  }))
  default     = []
  nullable    = false
  description = "Extra custom (HTTPS-only, TLSv1.2) origins, e.g. an API Gateway regional endpoint. origin_path may carry the API stage (e.g. \"/api\")."

  validation {
    condition     = length(var.additional_origins) == length(distinct([for o in var.additional_origins : o.origin_id]))
    error_message = "additional_origins origin_id values must be unique."
  }
}

variable "ordered_cache_behaviors" {
  type = list(object({
    path_pattern             = string
    target_origin_id         = string
    allowed_methods          = optional(list(string), ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"])
    cached_methods           = optional(list(string), ["GET", "HEAD"])
    cache_policy_id          = optional(string, "4135ea2d-6df8-44a3-9df3-4b5a84be39ad")
    origin_request_policy_id = optional(string, "b689b0a8-53d0-40ab-baf2-68738e2966ac")
  }))
  default     = []
  nullable    = false
  description = "Path-routed behaviors ahead of the default S3 behavior, evaluated in list order. Defaults suit an API origin: Managed-CachingDisabled + Managed-AllViewerExceptHostHeader."

  validation {
    condition = alltrue([
      for b in var.ordered_cache_behaviors :
      contains([for o in var.additional_origins : o.origin_id], b.target_origin_id)
    ])
    error_message = "Every ordered_cache_behaviors target_origin_id must match an additional_origins origin_id."
  }
}
```

(The two managed-policy UUIDs are AWS-published static IDs: `Managed-CachingDisabled`, `Managed-AllViewerExceptHostHeader` — same pattern as the module's existing hardcoded `Managed-SecurityHeadersPolicy` ID.)

- [ ] **Step 2: Distribution changes in `aws-cloudfront.tf`**

Find `local.default_alias` (currently `format("%s.%s", module.label_site.dns_name, var.domain_zone_name)`) and change to:

```hcl
default_alias = var.use_apex_domain ? var.domain_zone_name : format("%s.%s", module.label_site.dns_name, var.domain_zone_name)
```

Inside `resource "aws_cloudfront_distribution" "site"`, after the existing S3 `origin` block add:

```hcl
  dynamic "origin" {
    for_each = { for o in var.additional_origins : o.origin_id => o }
    content {
      origin_id   = origin.value.origin_id
      domain_name = origin.value.domain_name
      origin_path = origin.value.origin_path

      custom_origin_config {
        http_port              = 80
        https_port             = 443
        origin_protocol_policy = "https-only"
        origin_ssl_protocols   = ["TLSv1.2"]
      }
    }
  }
```

After `default_cache_behavior` add (mirroring the default behavior's `response_headers_policy_id` local so security headers apply to API responses' viewer side too):

```hcl
  dynamic "ordered_cache_behavior" {
    for_each = var.ordered_cache_behaviors
    content {
      path_pattern             = ordered_cache_behavior.value.path_pattern
      target_origin_id         = ordered_cache_behavior.value.target_origin_id
      allowed_methods          = ordered_cache_behavior.value.allowed_methods
      cached_methods           = ordered_cache_behavior.value.cached_methods
      cache_policy_id          = ordered_cache_behavior.value.cache_policy_id
      origin_request_policy_id = ordered_cache_behavior.value.origin_request_policy_id
      viewer_protocol_policy   = "redirect-to-https"
      compress                 = true
    }
  }
```

(Match the exact local name used for the default behavior's response headers policy when wiring — read the file first; if the default behavior uses `response_headers_policy_id = local.response_headers_policy_id`, add the same line to the dynamic block.)

- [ ] **Step 3: Example** — `examples/api-behaviors/main.tf` copying `examples/simple` shape (ctx.tf verbatim from simple, new name `api-behaviors`), module block adding:

```hcl
  use_apex_domain = false
  additional_origins = [{
    origin_id   = "api"
    domain_name = "example-api.execute-api.us-east-1.amazonaws.com"
    origin_path = "/api"
  }]
  ordered_cache_behaviors = [
    { path_pattern = "/api/*", target_origin_id = "api" },
  ]
```

- [ ] **Step 4: Validate + docs**

Run: `terraform init -backend=false && terraform validate` (root and example) — expect `Success!`
Run: `terraform-docs markdown table --output-file README.md .` then `npx prettier --write README.md` (the #159/#160 prettier trap).
Run: `tflint` — expect clean.

- [ ] **Step 5: Commit (signed), push, open PR**

```bash
git add variables.tf aws-cloudfront.tf README.md examples/
git commit -S -m "feat: additional origins + ordered cache behaviors + apex domain

Lets the distribution front more than the S3 origin (e.g. /api/* to an API
Gateway regional endpoint) and serve the zone apex. Behavior defaults are
the AWS managed CachingDisabled + AllViewerExceptHostHeader policies."
git push -u origin kitten/additional-origins
gh pr create --title "feat: additional origins, ordered cache behaviors, apex domain" --body "<what/why, @oldmanbendobot review>"
```

Verify `git log -1 --format='%an <%ae> | %GK'` shows `code kitten <yourcodekitten@gmail.com> | F2060B93112D9ACF` (fresh clone — set nothing, just check).

- [ ] **Step 6: Watch module CI; @oldmanbendobot for review.** Task complete when PR is open + CI green (merge/release is OMBB's; Task 7 pins the git ref meanwhile).

---

### Task 2: Stack skeleton — versions, backend, provider, variables, context

**Files (all under `terraform/` in bendobundles, branch `kitten/plan4-terraform`):**
- Create: `tf-versions.tf`, `tf-backend.tf`, `backend.hcl.example`, `tf-provider.tf`, `tf-variables.tf`, `main.tf`, `.gitignore` (append), `tf-outputs.tf` (empty-for-now header comment)

**Interfaces:**
- Produces: `module.context.shared` (every later task's `context`), `var.region`, `var.domain_zone_name`, `var.domain_zone_id`, `var.admin_password_hash`, `var.discord_webhook_url`, `local` label pattern via per-resource `module "label_*"` blocks in later tasks.

- [ ] **Step 1: `tf-versions.tf`**

```hcl
terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source                = "hashicorp/aws"
      version               = "~> 6.0"
      configuration_aliases = [aws.route53]
    }
  }
}
```

- [ ] **Step 2: `tf-backend.tf` + `backend.hcl.example`**

```hcl
# Partial backend — ben supplies values at init time:
#   terraform init -backend-config=backend.hcl
# (copy backend.hcl.example to backend.hcl and adjust; backend.hcl is gitignored)
terraform {
  backend "s3" {}
}
```

`backend.hcl.example`:

```hcl
bucket               = "bd-prod-ue1-tfstate-store" # ben: confirm your real state bucket
key                  = "terraform.tfstate"
kms_key_id           = "alias/aws/s3"
region               = "us-east-1"
use_lockfile         = true # S3-native locking (TF >= 1.10); no dynamodb_table
workspace_key_prefix = "bendobundles"
```

- [ ] **Step 3: `tf-provider.tf`**

```hcl
provider "aws" {
  allowed_account_ids = [var.aws_account_id]
  region              = var.region
}

# Route53 zone may live outside this account (org pattern) — pass-through alias.
# If the zone is in the same account, leave route53_profile null.
provider "aws" {
  alias               = "route53"
  allowed_account_ids = var.route53_profile == null ? [var.aws_account_id] : null
  region              = var.region
  profile             = var.route53_profile
}
```

- [ ] **Step 4: `tf-variables.tf`**

```hcl
variable "aws_account_id" {
  type        = string
  description = "Account this stack deploys into (guard against wrong-profile applies)."
}

variable "region" {
  type        = string
  default     = "us-east-1"
  description = "Sole region. CloudFront ACM requires us-east-1; everything colocates."
}

variable "namespace" {
  type        = string
  default     = "bd"
  description = "Org namespace for context/labels."
}

variable "role" {
  type        = string
  default     = "production"
  description = "Context role."
}

variable "domain_zone_name" {
  type        = string
  default     = "bendobundles.com"
  description = "Route53 zone serving the site."
}

variable "domain_zone_id" {
  type        = string
  description = "Route53 hosted zone ID for domain_zone_name."
}

variable "route53_profile" {
  type        = string
  default     = null
  description = "AWS profile for the account holding the Route53 zone, if different."
}

variable "admin_password_hash" {
  type        = string
  sensitive   = true
  description = "Argon2 PHC string for the admin password (generate: `echo -n 'pw' | argon2 \"$(openssl rand -base64 16)\" -id -e`). Stored as SSM SecureString; admin-api refuses to boot without it."
}

variable "discord_webhook_url" {
  type        = string
  default     = null
  sensitive   = true
  description = "Optional Discord webhook for cookie-death pings. Null disables (fulfillment treats a missing param as webhooks-off)."
}

variable "sync_schedule_expression" {
  type        = string
  default     = "cron(0 9 * * ? *)" # 09:00 UTC daily = pre-dawn US-East
  description = "EventBridge schedule for the daily humble sync."
}
```

- [ ] **Step 5: `main.tf` (context only — labels live next to their resources)**

```hcl
module "context" {
  source    = "bendoerr-terraform-modules/context/null"
  version   = "0.5.2"
  namespace = var.namespace
  role      = var.role
  region    = var.region
  project   = "bendobundles"
}
```

- [ ] **Step 6: `.gitignore` (repo root, append)**

```
terraform/.terraform/
terraform/backend.hcl
terraform/artifacts/
terraform/*.tfplan
```

(`.terraform.lock.hcl` IS committed — org pattern per infra-testing-sandbox.)

- [ ] **Step 7: Validate + commit**

Run: `cd terraform && terraform init -backend=false && terraform validate && terraform fmt -check`
Expected: `Success! The configuration is valid.` (context module downloads).

```bash
git add terraform/ .gitignore
git commit -S -m "feat(terraform): stack skeleton — versions, partial backend, providers, context"
```

---

### Task 3: DynamoDB table + SSM parameters

**Files:**
- Create: `terraform/aws-dynamodb.tf`, `terraform/aws-ssm.tf`
- Modify: `terraform/tf-outputs.tf`

**Interfaces:**
- Consumes: `module.context.shared`.
- Produces: `aws_dynamodb_table.this` (`.name`, `.arn`), `aws_ssm_parameter.admin_hash` (`.name`, `.arn`), `aws_ssm_parameter.humble_cookie` (`.name`, `.arn`), `aws_ssm_parameter.discord_webhook[0]` (count-gated; `local.discord_webhook_param_name`, `local.discord_webhook_param_arn` non-null only when enabled).

- [ ] **Step 1: `aws-dynamodb.tf`** (mirrors `dynamo::Store::create_table_for_tests` + TTL)

```hcl
module "label_table" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "table"
}

resource "aws_dynamodb_table" "this" {
  name         = module.label_table.id
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "pk"
  range_key    = "sk"

  attribute {
    name = "pk"
    type = "S"
  }
  attribute {
    name = "sk"
    type = "S"
  }
  attribute {
    name = "gsi1pk"
    type = "S"
  }
  attribute {
    name = "gsi1sk"
    type = "S"
  }
  attribute {
    name = "gsi2pk"
    type = "S"
  }
  attribute {
    name = "gsi2sk"
    type = "S"
  }

  global_secondary_index {
    name            = "listable"
    hash_key        = "gsi1pk"
    range_key       = "gsi1sk"
    projection_type = "ALL"
  }

  global_secondary_index {
    name            = "pending-claims"
    hash_key        = "gsi2pk"
    range_key       = "gsi2sk"
    projection_type = "ALL"
  }

  # Sessions carry a numeric `ttl` epoch (schema.rs writes it; code also checks
  # expiry itself, so TTL lag is harmless). This is the "terraform will enable
  # it in plan 4" note in dynamo/src/schema.rs.
  ttl {
    attribute_name = "ttl"
    enabled        = true
  }

  point_in_time_recovery {
    enabled = true
  }

  tags = module.label_table.tags
}
```

- [ ] **Step 2: `aws-ssm.tf`**

```hcl
module "label_param" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "param"
}

# Admin password hash — terraform owns the value (it is a HASH, not the
# password; the state bucket is private + encrypted). admin-api reads it once
# at boot and refuses to start without it.
resource "aws_ssm_parameter" "admin_hash" {
  name  = "/${module.label_param.id}/admin-hash"
  type  = "SecureString"
  value = var.admin_password_hash
  tags  = module.label_param.tags
}

# Humble session cookie — terraform creates the CONTAINER only; the value is
# owned by admin-api's paste flow (PutParameter overwrite). Placeholder value
# fails humble auth cleanly until ben pastes a real cookie in /admin/ops.
resource "aws_ssm_parameter" "humble_cookie" {
  name  = "/${module.label_param.id}/humble-cookie"
  type  = "SecureString"
  value = "UNSET"
  tags  = module.label_param.tags

  lifecycle {
    ignore_changes = [value]
  }
}

resource "aws_ssm_parameter" "discord_webhook" {
  count = var.discord_webhook_url == null ? 0 : 1
  name  = "/${module.label_param.id}/discord-webhook"
  type  = "String"
  value = var.discord_webhook_url
  tags  = module.label_param.tags
}

locals {
  discord_webhook_param_name = var.discord_webhook_url == null ? null : aws_ssm_parameter.discord_webhook[0].name
  discord_webhook_param_arn  = var.discord_webhook_url == null ? null : aws_ssm_parameter.discord_webhook[0].arn
}
```

- [ ] **Step 3: `tf-outputs.tf` additions**

```hcl
output "dynamodb_table_name" {
  value = aws_dynamodb_table.this.name
}
```

- [ ] **Step 4: Validate + commit**

Run: `cd terraform && terraform validate && terraform fmt -check` — expect `Success!`

```bash
git add terraform/
git commit -S -m "feat(terraform): dynamo single table (2 sparse GSIs, ttl) + 3 SSM params"
```

---

### Task 4: Build tooling + three lambdas

**Files:**
- Create: `terraform/build.sh`, `terraform/aws-lambda.tf`
- Modify: `terraform/tf-outputs.tf`

**Interfaces:**
- Consumes: `aws_dynamodb_table.this`, `aws_ssm_parameter.*`, `local.discord_webhook_param_*` (Task 3).
- Produces: `module.lambda_public_api`, `module.lambda_admin_api`, `module.lambda_fulfillment` — each exposing `lambda_function_arn`, `lambda_function_name`, `iam_role_arn` (Tasks 5/6 consume). Artifacts land at `terraform/artifacts/{public-api,admin-api,fulfillment}.zip`.

- [ ] **Step 1: `terraform/build.sh`** (chmod +x)

```bash
#!/usr/bin/env bash
# Build the three lambda zips + the web bundle. Requires: cargo-lambda
# (https://www.cargo-lambda.info), zig or the cross toolchain it manages,
# node 22. Run from anywhere; paths are repo-relative.
set -euo pipefail
cd "$(dirname "$0")/.."

for bin in public-api admin-api fulfillment; do
  cargo lambda build --release --arm64 --output-format zip --bin "$bin"
done

mkdir -p terraform/artifacts
for bin in public-api admin-api fulfillment; do
  cp "target/lambda/$bin/bootstrap.zip" "terraform/artifacts/$bin.zip"
done

(cd web && npm ci && npm run build)

echo "artifacts:"
ls -la terraform/artifacts/
echo "web bundle: web/dist/"
```

- [ ] **Step 2: `aws-lambda.tf`**

```hcl
# ── fulfillment — the ONLY component that can read the humble session ────────
module "lambda_fulfillment" {
  source  = "bendoerr-terraform-modules/lambda/aws"
  version = "0.3.0"
  context = module.context.shared
  name    = "fulfillment"

  description   = "Sole humble-toucher: gift fulfillment, daily sync, cookie validation"
  filename      = "${path.module}/artifacts/fulfillment.zip"
  handler       = "bootstrap"
  runtime       = "provided.al2023"
  architectures = ["arm64"]
  memory_size   = 256
  timeout       = 900 # first sync backfills ~15 years of orders, paced

  environment = {
    variables = merge(
      {
        TABLE_NAME          = aws_dynamodb_table.this.name
        HUMBLE_COOKIE_PARAM = aws_ssm_parameter.humble_cookie.name
      },
      local.discord_webhook_param_name == null ? {} : {
        DISCORD_WEBHOOK_PARAM = local.discord_webhook_param_name
      }
    )
  }

  addl_inline_policies = {
    dynamo = data.aws_iam_policy_document.dynamo_rw.json
    ssm = jsonencode({
      Version = "2012-10-17"
      Statement = concat(
        [{
          Effect   = "Allow"
          Action   = ["ssm:GetParameter"]
          Resource = [aws_ssm_parameter.humble_cookie.arn]
        }],
        local.discord_webhook_param_arn == null ? [] : [{
          Effect   = "Allow"
          Action   = ["ssm:GetParameter"]
          Resource = [local.discord_webhook_param_arn]
        }]
      )
    })
  }
}

# ── public-api — friend surface; ZERO ssm access (trust boundary) ────────────
module "lambda_public_api" {
  source  = "bendoerr-terraform-modules/lambda/aws"
  version = "0.3.0"
  context = module.context.shared
  name    = "public-api"

  description   = "Friend surface: link view + claim intake"
  filename      = "${path.module}/artifacts/public-api.zip"
  handler       = "bootstrap"
  runtime       = "provided.al2023"
  architectures = ["arm64"]
  memory_size   = 256
  timeout       = 29 # API Gateway's integration ceiling; gift invoke must fit inside

  environment = {
    variables = {
      TABLE_NAME     = aws_dynamodb_table.this.name
      FULFILLMENT_FN = module.lambda_fulfillment.lambda_function_name
    }
  }

  addl_inline_policies = {
    dynamo = data.aws_iam_policy_document.dynamo_rw.json
    invoke_fulfillment = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect   = "Allow"
        Action   = ["lambda:InvokeFunction"]
        Resource = [module.lambda_fulfillment.lambda_function_arn]
      }]
    })
  }
}

# ── admin-api — ben surface ───────────────────────────────────────────────────
module "lambda_admin_api" {
  source  = "bendoerr-terraform-modules/lambda/aws"
  version = "0.3.0"
  context = module.context.shared
  name    = "admin-api"

  description   = "Admin surface: login, links, hidden toggles, cookie paste, sync-now"
  filename      = "${path.module}/artifacts/admin-api.zip"
  handler       = "bootstrap"
  runtime       = "provided.al2023"
  architectures = ["arm64"]
  memory_size   = 256
  timeout       = 29

  environment = {
    variables = {
      TABLE_NAME          = aws_dynamodb_table.this.name
      FULFILLMENT_FN      = module.lambda_fulfillment.lambda_function_name
      ADMIN_HASH_PARAM    = aws_ssm_parameter.admin_hash.name
      HUMBLE_COOKIE_PARAM = aws_ssm_parameter.humble_cookie.name
    }
  }

  addl_inline_policies = {
    dynamo = data.aws_iam_policy_document.dynamo_rw.json
    invoke_fulfillment = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect   = "Allow"
        Action   = ["lambda:InvokeFunction"]
        Resource = [module.lambda_fulfillment.lambda_function_arn]
      }]
    })
    # paste flow: snapshot old cookie (Get) + write new (Put); hash: boot read
    ssm = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["ssm:GetParameter"]
          Resource = [aws_ssm_parameter.admin_hash.arn, aws_ssm_parameter.humble_cookie.arn]
        },
        {
          Effect   = "Allow"
          Action   = ["ssm:PutParameter"]
          Resource = [aws_ssm_parameter.humble_cookie.arn]
        }
      ]
    })
  }
}

# Shared dynamo policy: full data-plane on the table + its indexes.
# TransactWriteItems authorizes as the underlying item ops.
data "aws_iam_policy_document" "dynamo_rw" {
  statement {
    effect = "Allow"
    actions = [
      "dynamodb:BatchGetItem",
      "dynamodb:ConditionCheckItem",
      "dynamodb:DeleteItem",
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:Query",
      "dynamodb:Scan",
      "dynamodb:UpdateItem",
    ]
    resources = [
      aws_dynamodb_table.this.arn,
      "${aws_dynamodb_table.this.arn}/index/*",
    ]
  }
}
```

- [ ] **Step 3: Validate.** `terraform validate` doesn't read the zip files (`source_code_hash` is computed at plan), so this passes with no artifacts present. Run: `cd terraform && terraform validate && terraform fmt -check` — expect `Success!`

- [ ] **Step 4: Sanity-build once locally IF cargo-lambda is installed** (`command -v cargo-lambda`); otherwise note in the task report that CI (Task 8) is the build proof. Do NOT install toolchains on this box.

- [ ] **Step 5: Commit**

```bash
git add terraform/
git commit -S -m "feat(terraform): three lambdas via org module — env contract, trust-boundary IAM, build.sh"
```

---

### Task 5: API Gateway (REST, OpenAPI body, both surfaces)

**Files:**
- Create: `terraform/aws-apigateway.tf`
- Modify: `terraform/tf-outputs.tf`

**Interfaces:**
- Consumes: `module.lambda_public_api.lambda_function_arn/.lambda_function_name`, `module.lambda_admin_api.…` (Task 4), `var.region`.
- Produces: `module.apigateway.rest_api_id`, `module.apigateway.stage_name`, `module.apigateway.rest_api_execution_arn`; `local.api_origin_domain` = `"${module.apigateway.rest_api_id}.execute-api.${var.region}.amazonaws.com"` (Task 7 consumes as the API origin).

- [ ] **Step 1: `aws-apigateway.tf`**

```hcl
module "apigateway" {
  source  = "bendoerr-terraform-modules/apigateway/aws"
  version = "1.1.1"
  context = module.context.shared
  name    = "api"

  description = "bendobundles API: /api/* -> public-api, /admin/api/* -> admin-api"

  endpoint_configuration = {
    types = ["REGIONAL"] # CloudFront sits in front; EDGE would double-CDN
  }

  stage_config = {
    name = "live"
  }

  # Personal-scale throttling — spec §7 wants rate-limited token lookups.
  method_settings = {
    "*/*" = {
      throttling_rate_limit  = 25
      throttling_burst_limit = 50
    }
  }

  openapi_config = jsonencode({
    openapi = "3.0.1"
    info = {
      title   = "bendobundles"
      version = "1.0"
    }
    paths = {
      "/api/{proxy+}" = {
        x-amazon-apigateway-any-method = {
          parameters = [{
            name     = "proxy"
            in       = "path"
            required = true
            schema   = { type = "string" }
          }]
          x-amazon-apigateway-integration = {
            uri                 = "arn:aws:apigateway:${var.region}:lambda:path/2015-03-31/functions/${module.lambda_public_api.lambda_function_arn}/invocations"
            type                = "aws_proxy"
            httpMethod          = "POST"
            passthroughBehavior = "when_no_match"
            timeoutInMillis     = 29000
          }
          responses = { "200" = { description = "proxied" } }
        }
      }
      "/admin/api/{proxy+}" = {
        x-amazon-apigateway-any-method = {
          parameters = [{
            name     = "proxy"
            in       = "path"
            required = true
            schema   = { type = "string" }
          }]
          x-amazon-apigateway-integration = {
            uri                 = "arn:aws:apigateway:${var.region}:lambda:path/2015-03-31/functions/${module.lambda_admin_api.lambda_function_arn}/invocations"
            type                = "aws_proxy"
            httpMethod          = "POST"
            passthroughBehavior = "when_no_match"
            timeoutInMillis     = 29000
          }
          responses = { "200" = { description = "proxied" } }
        }
      }
    }
  })
}

# Module does NOT create integration permissions (confirmed) — caller wires them.
resource "aws_lambda_permission" "apigw_public" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_public_api.lambda_function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${module.apigateway.rest_api_execution_arn}/*/*/*"
}

resource "aws_lambda_permission" "apigw_admin" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_admin_api.lambda_function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${module.apigateway.rest_api_execution_arn}/*/*/*"
}

locals {
  api_origin_domain = "${module.apigateway.rest_api_id}.execute-api.${var.region}.amazonaws.com"
}
```

Routing note (verified against lambda_http + REST aws_proxy): a viewer request to `https://bendobundles.com/api/l/x` hits CloudFront behavior `/api/*` → origin `{api}.execute-api…` with `origin_path = "/live"` → API GW strips the stage → the proxy event's `path` is `/api/l/x` → axum's routes (`/api/l/:token`) match unchanged. Same for `/admin/api/*`. `/admin` itself (no `/api`) stays on the S3 origin → SPA. Session cookie (`Path=/admin; SameSite=Strict`) flows because origin + viewer domain are identical and the AllViewerExceptHostHeader policy forwards Cookie headers.

- [ ] **Step 2: `tf-outputs.tf` additions**

```hcl
output "api_stage_invoke_url" {
  value = module.apigateway.stage_invoke_url
}
```

- [ ] **Step 3: Validate + commit**

Run: `cd terraform && terraform validate && terraform fmt -check` — expect `Success!`

```bash
git add terraform/
git commit -S -m "feat(terraform): API Gateway REST — proxy both surfaces, stage throttling, invoke permissions"
```

---

### Task 6: EventBridge daily sync

**Files:**
- Create: `terraform/aws-eventbridge.tf`

**Interfaces:**
- Consumes: `module.lambda_fulfillment.lambda_function_arn/.lambda_function_name`, `var.sync_schedule_expression`.

- [ ] **Step 1: `aws-eventbridge.tf`**

```hcl
module "label_sync" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "sync"
}

# Default EventBridge envelope carries "source": "aws.events" — fulfillment's
# handler routes exactly that to FulfillRequest::Sync (main.rs), so no
# input transformer is needed or wanted.
resource "aws_cloudwatch_event_rule" "sync" {
  name                = module.label_sync.id
  description         = "Daily humble library sync + parked-claim reconcile"
  schedule_expression = var.sync_schedule_expression
  tags                = module.label_sync.tags
}

resource "aws_cloudwatch_event_target" "sync" {
  rule = aws_cloudwatch_event_rule.sync.name
  arn  = module.lambda_fulfillment.lambda_function_arn
}

resource "aws_lambda_permission" "eventbridge_sync" {
  statement_id  = "AllowEventBridgeInvoke"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_fulfillment.lambda_function_name
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.sync.arn
}
```

- [ ] **Step 2: Validate + commit**

Run: `cd terraform && terraform validate && terraform fmt -check` — expect `Success!`

```bash
git add terraform/
git commit -S -m "feat(terraform): eventbridge daily sync -> fulfillment (default envelope routes to Sync)"
```

---

### Task 7: CloudFront + S3 via the enhanced module

**Files:**
- Create: `terraform/aws-cloudfront.tf`, `terraform/deploy-web.sh`
- Modify: `terraform/tf-outputs.tf`

**Interfaces:**
- Consumes: Task 1's new module vars; `local.api_origin_domain` + `module.apigateway.stage_name` (Task 5).
- Produces: outputs `site_url`, `s3_bucket_id`, `cloudfront_distribution_id` (deploy-web.sh + ben consume).

- [ ] **Step 1: `aws-cloudfront.tf`**

```hcl
# Pin the PR branch ref until OMBB releases the enhancement (Task 1); swap to
# `source = "bendoerr-terraform-modules/cloudfront-and-s3-origin/aws", version = "0.5.0"`
# when tagged. Tracked in the PR body checklist.
module "site" {
  source  = "git::https://github.com/bendoerr-terraform-modules/terraform-aws-cloudfront-and-s3-origin.git?ref=kitten/additional-origins"
  context = module.context.shared
  name    = "site"

  domain_zone_name = var.domain_zone_name
  domain_zone_id   = var.domain_zone_id
  use_apex_domain  = true

  # SPA deep links: /l/<token> and /admin/* are client routes; S3 objects don't
  # exist there. 403/404 -> 200 /index.html (module knob; plan-3 carry).
  enable_spa_error_handling = true
  security_headers          = "managed"

  additional_origins = [{
    origin_id   = "api"
    domain_name = local.api_origin_domain
    origin_path = "/${module.apigateway.stage_name}"
  }]

  ordered_cache_behaviors = [
    { path_pattern = "/api/*", target_origin_id = "api" },
    { path_pattern = "/admin/api/*", target_origin_id = "api" },
  ]

  providers = {
    aws.route53 = aws.route53
  }
}
```

- [ ] **Step 2: `deploy-web.sh`** (chmod +x)

```bash
#!/usr/bin/env bash
# Publish the SPA: sync web/dist to the site bucket + invalidate CloudFront.
# Ben runs this after `terraform apply` and after any web change. His creds.
set -euo pipefail
cd "$(dirname "$0")"

BUCKET="$(terraform output -raw s3_bucket_id)"
DIST_ID="$(terraform output -raw cloudfront_distribution_id)"

test -d ../web/dist || { echo "web/dist missing — run ./build.sh first" >&2; exit 1; }

aws s3 sync ../web/dist "s3://$BUCKET" --delete
aws cloudfront create-invalidation --distribution-id "$DIST_ID" --paths "/*"
echo "deployed to https://$(terraform output -raw site_url)"
```

- [ ] **Step 3: `tf-outputs.tf` additions**

```hcl
output "site_url" {
  value = module.site.cloudfront_distribution_alias_domain_name
}

output "s3_bucket_id" {
  value = module.site.s3_bucket_id
}

output "cloudfront_distribution_id" {
  value = module.site.cloudfront_distribution_id
}
```

- [ ] **Step 4: Validate + commit**

Run: `cd terraform && terraform init -backend=false && terraform validate && terraform fmt -check` (init refetches the git-ref module) — expect `Success!`

```bash
git add terraform/
git commit -S -m "feat(terraform): cloudfront+s3 site — apex, SPA rewrite, /api routing to API GW"
```

---

### Task 8: CI (terraform gates + artifact builds) + README runbook

**Files:**
- Modify: `.github/workflows/ci.yml` (two new jobs; keep existing `test`/`web` untouched)
- Create: `terraform/README.md`

**Interfaces:**
- Consumes: everything prior; produces the runbook ben follows.

- [ ] **Step 1: append to `.github/workflows/ci.yml`** (same SHA-pin style as existing jobs; note the step-security-on-private-repo caveat comment already in the file):

```yaml
  terraform:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1
        with:
          persist-credentials: false
      - uses: hashicorp/setup-terraform@b9cd54a3c349d3f38e8881555d616ced269862dd # v3.1.2
        with:
          terraform_version: "1.12.2"
      - run: terraform -chdir=terraform fmt -check -diff
      - run: terraform -chdir=terraform init -backend=false
      - run: terraform -chdir=terraform validate

  artifacts:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@34e114876b0b11c390a56381ad16ebd13914f8d5 # v4.3.1
        with:
          persist-credentials: false
      - uses: dtolnay/rust-toolchain@4be7066ada62dd38de10e7b70166bc74ed198c30 # stable
      - uses: Swatinem/rust-cache@23869a5bd66c73db3c0ac40331f3206eb23791dc # v2.9.1
      - name: install cargo-lambda
        run: pip3 install cargo-lambda
      - run: |
          for bin in public-api admin-api fulfillment; do
            cargo lambda build --release --arm64 --output-format zip --bin "$bin"
          done
      - uses: actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4.4.0
        with:
          node-version: '22'
          cache: npm
          cache-dependency-path: web/package-lock.json
      - run: npm ci && npm run build
        working-directory: web
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2
        with:
          name: lambda-zips
          path: target/lambda/*/bootstrap.zip
          retention-days: 14
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4.6.2
        with:
          name: web-dist
          path: web/dist/
          retention-days: 14
```

(Implementer: verify the two new-action SHAs against the tags before committing — `gh api repos/hashicorp/setup-terraform/git/ref/tags/v3.1.2` and `gh api repos/actions/upload-artifact/git/ref/tags/v4.6.2`; if they don't resolve, pin whatever the latest release SHA actually is and note it in the report. cargo-lambda via pip is the documented CI install.)

- [ ] **Step 2: `terraform/README.md`** — write these sections, complete prose (this is ben's runbook):
  1. **What this deploys** — one paragraph + the spec §3 diagram reference.
  2. **One-time bootstrap** — copy `backend.hcl.example` → `backend.hcl`; generate the admin hash (`echo -n 'the-password' | argon2 "$(openssl rand -base64 16)" -id -e`); `terraform init -backend-config=backend.hcl`; `terraform workspace new production` (or default).
  3. **Deploy loop** — `./build.sh` → `terraform plan -var-file=production.tfvars -out=tf.plan` → `terraform apply tf.plan` → `./deploy-web.sh`. Example `production.tfvars` block with every required var (aws_account_id, domain_zone_id, admin_password_hash via `TF_VAR_admin_password_hash` env recommended instead of the file).
  4. **After first deploy** — paste the humble cookie in `/admin` → ops; run sync-now; the VERIFY checklist (below) at first real gifting.
  5. **VERIFY at first real gifting** (plan-2/3 carries, ben-triggered, live humble): keyindex handling on a real multi-key order; RedeemRefused/AmbiguousRedeem park behavior; gift URL renders + copies on a real phone; cookie-death Discord ping fires (webhook param set).
  6. **Module pin note** — `module "site"` rides the PR branch until cloudfront-and-s3-origin vX releases; swap source+version then.

- [ ] **Step 3: actionlint** — Run: `actionlint` (if installed; else note). Expect clean.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/ci.yml terraform/README.md
git commit -S -m "feat(ci): terraform gates + lambda/web artifact builds; terraform runbook for ben"
```

---

### Task 9: Final review + PR fork-to-ready

- [ ] **Step 1:** Full self-check: `terraform fmt -check`, `init -backend=false`, `validate`; re-read every file against the Global Constraints (esp. env-var names EXACT, `HUMBLE_COOKIE_PARAM` identical in admin-api + fulfillment env blocks, trust-boundary IAM shape).
- [ ] **Step 2:** Dispatch an adversarial reviewer subagent over the whole diff (fresh eyes, spec + this plan in hand). Fix findings.
- [ ] **Step 3:** Push branch, open PR: title `plan 4: terraform — the whole stack as legos`, body maps spec§→files, links the module-enhancement PR as a dependency, lists the ben-decisions (apex, backend values, account id) and the VERIFY checklist. Watch CI to green. Reply to review rounds as they come (ben's automated reviewer has receipts 24 deep — expect rounds).
- [ ] **Step 4:** Update `.superpowers/sdd/progress.md` + journal + review-log; PR is ben's to approve; I merge when APPROVED+CLEAN per Hard Rule #1.

## Self-review notes (done at write time)

- Spec coverage: §3 architecture (T4/5/7), §4 data model table (T3), §5/§6 handled in code (plans 1-2; terraform only wires), §7 rate-limiting (T5 method_settings), §8 admin (code; T5 routes it), §9 sync+cookie lifecycle (T3 params + T6 schedule + webhook param), §10 frontend hosting (T7), §11 testing (T8 CI gates; terratest lives in the module repo, not the stack), §12 layout + ben-applies (T2 backend + T8 runbook), §13/§14 n/a.
- Trust boundary honesty: admin-api Get on humble-cookie is real (paste-rollback flow) — documented in Global Constraints rather than pretending public/admin symmetry.
- Type consistency: `module.lambda_*` output names match terraform-aws-lambda v0.3.0 outputs; `stage_name`/`rest_api_id`/`rest_api_execution_arn` match apigateway v1.1.1 outputs; new module vars in T7 match T1's declared shapes exactly.
- Known risk: Task 1 example context version + exact local names inside `aws-cloudfront.tf` must be read from the actual file at implementation time (marked in-task). Managed policy UUIDs are AWS-global constants.
