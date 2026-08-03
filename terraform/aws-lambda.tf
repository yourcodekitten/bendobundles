# ── fulfillment — the ONLY component that can read the humble session ────────
module "lambda_fulfillment" {
  source  = "bendoerr-terraform-modules/lambda/aws"
  version = "0.4.0"
  context = module.context.shared
  name    = "fulfillment"

  description   = "Sole humble-toucher: gift fulfillment, daily sync, cookie validation"
  filename      = "${path.module}/artifacts/fulfillment.zip"
  handler       = "bootstrap"
  runtime       = "provided.al2023"
  architectures = ["arm64"]
  # Cap the exec role's effective permissions so the least-privilege deploy role can safely
  # manage its inline policies (see var.lambda_permissions_boundary_arn). Null = unbounded.
  permissions_boundary = var.lambda_permissions_boundary_arn
  memory_size          = 256
  timeout              = 900 # first sync backfills ~15 years of orders, paced

  environment_variables = merge(
    {
      TABLE_NAME          = aws_dynamodb_table.this.name
      HUMBLE_COOKIE_PARAM = aws_ssm_parameter.humble_cookie.name
      STEAM_KEY_PARAM     = aws_ssm_parameter.steam_web_api_key.name
    },
    local.discord_webhook_param_name == null ? {} : {
      DISCORD_WEBHOOK_PARAM = local.discord_webhook_param_name
    },
    local.humble_step_up_env
  )

  addl_inline_policies = {
    dynamo = data.aws_iam_policy_document.dynamo_rw.json
    ssm = jsonencode({
      Version = "2012-10-17"
      Statement = concat(
        [{
          Effect = "Allow"
          Action = ["ssm:GetParameter"]
          # The cookie plus steam key plus, when step-up is enabled, the password + TOTP seed.
          Resource = concat([aws_ssm_parameter.humble_cookie.arn, aws_ssm_parameter.steam_web_api_key.arn], local.humble_step_up_param_arns)
        }],
        # Self-login writes the refreshed session back to the cookie param (fulfillment is the
        # SOLE writer now that the admin cookie-paste flow is retired). Scoped to the cookie
        # param only — never the password/TOTP seeds.
        [{
          Effect   = "Allow"
          Action   = ["ssm:PutParameter"]
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

# ── public-api — friend surface; steam key only, never the humble session (trust boundary) ──
module "lambda_public_api" {
  source  = "bendoerr-terraform-modules/lambda/aws"
  version = "0.4.0"
  context = module.context.shared
  name    = "public-api"

  description   = "Friend surface: link view + claim intake"
  filename      = "${path.module}/artifacts/public-api.zip"
  handler       = "bootstrap"
  runtime       = "provided.al2023"
  architectures = ["arm64"]
  # Cap the exec role's effective permissions so the least-privilege deploy role can safely
  # manage its inline policies (see var.lambda_permissions_boundary_arn). Null = unbounded.
  permissions_boundary = var.lambda_permissions_boundary_arn
  memory_size          = 256
  timeout              = 29 # API Gateway's integration ceiling; gift invoke must fit inside

  environment_variables = {
    TABLE_NAME      = aws_dynamodb_table.this.name
    FULFILLMENT_FN  = module.lambda_fulfillment.lambda_function_name
    STEAM_KEY_PARAM = aws_ssm_parameter.steam_web_api_key.name
    # Public site origin for building the steam OpenID return_to URL — server-trusted,
    # never derived from request headers. CloudFront routes /api/* from the apex back here.
    BASE_URL = "https://${var.domain_zone_name}"
    # REST API GW puts the stage in the path; lambda_http PREPENDS it (turning
    # /api/l/x into /live/api/l/x), which axum then 404s. This flag makes
    # lambda_http return the stage-less path so the routes match. Verified
    # against lambda_http 0.14 request.rs::apigw_path_with_stage.
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH = "true"
  }

  addl_inline_policies = {
    dynamo             = data.aws_iam_policy_document.dynamo_rw_public.json
    invoke_fulfillment = data.aws_iam_policy_document.invoke_fulfillment.json
    ssm = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["ssm:GetParameter"]
          Resource = [aws_ssm_parameter.steam_web_api_key.arn]
        }
      ]
    })
  }
}

# ── admin-api — ben surface ───────────────────────────────────────────────────
module "lambda_admin_api" {
  source  = "bendoerr-terraform-modules/lambda/aws"
  version = "0.4.0"
  context = module.context.shared
  name    = "admin-api"

  description   = "Admin surface: login, links, hidden toggles, sync-now"
  filename      = "${path.module}/artifacts/admin-api.zip"
  handler       = "bootstrap"
  runtime       = "provided.al2023"
  architectures = ["arm64"]
  # Cap the exec role's effective permissions so the least-privilege deploy role can safely
  # manage its inline policies (see var.lambda_permissions_boundary_arn). Null = unbounded.
  permissions_boundary = var.lambda_permissions_boundary_arn
  memory_size          = 256
  timeout              = 29

  environment_variables = {
    TABLE_NAME       = aws_dynamodb_table.this.name
    FULFILLMENT_FN   = module.lambda_fulfillment.lambda_function_name
    ADMIN_HASH_PARAM = aws_ssm_parameter.admin_hash.name
    STEAM_KEY_PARAM  = aws_ssm_parameter.steam_web_api_key.name
    # See public-api: strips the REST stage prefix so axum's /admin/api/* routes match.
    AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH = "true"
  }

  addl_inline_policies = {
    dynamo             = data.aws_iam_policy_document.dynamo_rw.json
    invoke_fulfillment = data.aws_iam_policy_document.invoke_fulfillment.json
    # hash: boot read only. The humble-cookie Get+Put the paste flow needed is gone —
    # fulfillment's self-login owns that param now.
    ssm = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["ssm:GetParameter"]
          Resource = [aws_ssm_parameter.admin_hash.arn, aws_ssm_parameter.steam_web_api_key.arn]
        }
      ]
    })
  }
}

# Shared invoke policy: both API lambdas call fulfillment with the same single
# statement — one definition, edited once.
data "aws_iam_policy_document" "invoke_fulfillment" {
  statement {
    effect    = "Allow"
    actions   = ["lambda:InvokeFunction"]
    resources = [module.lambda_fulfillment.lambda_function_arn]
  }
}

# Privileged dynamo policy — full data-plane on the table + its indexes.
# TransactWriteItems authorizes as the underlying item ops.
# Attached by admin-api (legitimately reads/writes SESSION# + Scans list_all_games/list_links) and
# fulfillment (internal invoke-only; Scans list_all_games, owns SYNC#). public-api gets the tighter
# dynamo_rw_public below (#84) — it is the unauthenticated internet-facing lambda.
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

# public-api dynamo policy (#84): the unauthenticated internet-facing lambda, scoped tighter than the
# shared dynamo_rw in two ways.
#   1. NO Scan. public-api never Scans (it Querys the `listable` GSI); and dynamodb:LeadingKeys cannot
#      constrain a Scan, so keeping Scan would make the SESSION# split below theater (a Scan reads
#      every item regardless of any key condition).
#   2. Explicit Deny on SESSION#*/SYNC#* leading keys. public-api never touches admin sessions or
#      sync-control items (audited: no session/sync store methods), so denying the key-specifying
#      actions there removes the "mint or read an admin session" blast radius a future public-api bug
#      could otherwise reach — the one place the trust-boundary split (public-api gets ZERO ssm) was
#      not mirrored on the data plane.
# Deny (not a LeadingKeys allowlist) is deliberate: an allowlist on the base pk breaks the `listable`
# GSI Query — a GSI query's LeadingKeys is the index key, not the base pk — whereas a Deny on
# SESSION#*/SYNC#* never matches that query's GAME# items, so it is GSI-safe. The allowed actions keep
# ConditionCheckItem (public-api's claim_game runs a TransactWriteItems) and DeleteItem
# (take_oidc_state) — dropping either would 403 a real path.
data "aws_iam_policy_document" "dynamo_rw_public" {
  statement {
    sid    = "DataPlaneNoScan"
    effect = "Allow"
    actions = [
      "dynamodb:BatchGetItem",
      "dynamodb:ConditionCheckItem",
      "dynamodb:DeleteItem",
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:Query",
      "dynamodb:UpdateItem",
    ]
    resources = [
      aws_dynamodb_table.this.arn,
      "${aws_dynamodb_table.this.arn}/index/*",
    ]
  }
  statement {
    sid    = "DenySessionAndSyncItems"
    effect = "Deny"
    actions = [
      "dynamodb:BatchGetItem",
      "dynamodb:ConditionCheckItem",
      "dynamodb:DeleteItem",
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:Query",
      "dynamodb:UpdateItem",
    ]
    resources = [aws_dynamodb_table.this.arn]
    condition {
      test     = "ForAnyValue:StringLike"
      variable = "dynamodb:LeadingKeys"
      values   = ["SESSION#*", "SYNC#*"]
    }
  }
}
