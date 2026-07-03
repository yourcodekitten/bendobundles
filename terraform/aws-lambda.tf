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

  environment_variables = merge(
    {
      TABLE_NAME          = aws_dynamodb_table.this.name
      HUMBLE_COOKIE_PARAM = aws_ssm_parameter.humble_cookie.name
    },
    local.discord_webhook_param_name == null ? {} : {
      DISCORD_WEBHOOK_PARAM = local.discord_webhook_param_name
    }
  )

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

  environment_variables = {
    TABLE_NAME     = aws_dynamodb_table.this.name
    FULFILLMENT_FN = module.lambda_fulfillment.lambda_function_name
  }

  addl_inline_policies = {
    dynamo             = data.aws_iam_policy_document.dynamo_rw.json
    invoke_fulfillment = data.aws_iam_policy_document.invoke_fulfillment.json
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

  environment_variables = {
    TABLE_NAME          = aws_dynamodb_table.this.name
    FULFILLMENT_FN      = module.lambda_fulfillment.lambda_function_name
    ADMIN_HASH_PARAM    = aws_ssm_parameter.admin_hash.name
    HUMBLE_COOKIE_PARAM = aws_ssm_parameter.humble_cookie.name
  }

  addl_inline_policies = {
    dynamo             = data.aws_iam_policy_document.dynamo_rw.json
    invoke_fulfillment = data.aws_iam_policy_document.invoke_fulfillment.json
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

# Shared invoke policy: both API lambdas call fulfillment with the same single
# statement — one definition, edited once.
data "aws_iam_policy_document" "invoke_fulfillment" {
  statement {
    effect    = "Allow"
    actions   = ["lambda:InvokeFunction"]
    resources = [module.lambda_fulfillment.lambda_function_arn]
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
