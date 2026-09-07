# ── kitten-app-boundary — permissions ceiling for roles the deploy role manages ─
# The deploy role may only create or empower IAM roles that carry THIS policy as
# their permissions boundary (enforced by iam:PermissionsBoundary conditions in
# iam-deploy-role.tf). Effective permissions = intersection(role policy, boundary),
# so even a deliberately-attached admin policy on an app role cannot exceed the
# app runtime's needs — that closes the attach-admin-inline-policy →
# UpdateFunctionCode → invoke-through-apigw escalation.
#
# This is a CAP, not a grant: statements here are deliberately a superset of what
# the app roles' own policies allow (those stay least-privilege); anything not
# allowed here is unreachable no matter what gets attached to a bounded role.
#
# Lives in THIS stack on purpose: the deploy role has no iam:*Policy* actions and
# its NeverTouchKittenIam Deny covers *kitten* policy ARNs, so it can never edit
# its own ceiling.
module "label_boundary" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "kitten-app-boundary"
}

resource "aws_iam_policy" "app_boundary" {
  name   = module.label_boundary.id
  policy = data.aws_iam_policy_document.app_boundary.json
  tags   = module.label_boundary.tags
}

data "aws_iam_policy_document" "app_boundary" {
  # CloudWatch Logs — lambda log groups are app-prefixed, but the API Gateway
  # account logging role writes API-Gateway-Execution-Logs_<api-id>/<stage>,
  # which is named by AWS. Region+account bounded; log data is not an
  # escalation surface.
  statement {
    sid    = "Logs"
    effect = "Allow"
    actions = [
      "logs:CreateLogGroup",
      "logs:CreateLogStream",
      "logs:PutLogEvents",
      "logs:DescribeLogGroups",
      "logs:DescribeLogStreams",
      "logs:GetLogEvents",
      "logs:FilterLogEvents",
    ]
    resources = ["arn:aws:logs:${local.region}:${local.account}:*"]
  }

  # DynamoDB data plane — the app table + indexes only.
  statement {
    sid    = "Dynamo"
    effect = "Allow"
    actions = [
      "dynamodb:BatchGetItem",
      "dynamodb:BatchWriteItem",
      "dynamodb:ConditionCheckItem",
      "dynamodb:DeleteItem",
      "dynamodb:GetItem",
      "dynamodb:PutItem",
      "dynamodb:Query",
      "dynamodb:Scan",
      "dynamodb:UpdateItem",
      "dynamodb:DescribeTable",
    ]
    resources = [
      "arn:aws:dynamodb:${local.region}:${local.account}:table/${local.app_prefix}*",
      "arn:aws:dynamodb:${local.region}:${local.account}:table/${local.app_prefix}*/index/*",
    ]
  }

  # SSM — app-namespace parameters (cookie read, self-login cookie persist, hash read).
  statement {
    sid    = "SsmAppParams"
    effect = "Allow"
    actions = [
      "ssm:GetParameter",
      "ssm:GetParameters",
      "ssm:PutParameter",
    ]
    resources = ["arn:aws:ssm:${local.region}:${local.account}:parameter/${local.app_prefix}*"]
  }

  # KMS only as reached THROUGH SSM (SecureString params) — a bounded role can
  # never call KMS directly against arbitrary keys.
  statement {
    sid    = "KmsViaSsm"
    effect = "Allow"
    actions = [
      "kms:Decrypt",
      "kms:Encrypt",
      "kms:GenerateDataKey",
    ]
    resources = ["*"]
    condition {
      test     = "StringEquals"
      variable = "kms:ViaService"
      values   = ["ssm.${local.region}.amazonaws.com"]
    }
  }

  # Lambda-to-lambda — the API lambdas invoke fulfillment.
  statement {
    sid       = "InvokeAppLambdas"
    effect    = "Allow"
    actions   = ["lambda:InvokeFunction"]
    resources = ["arn:aws:lambda:${local.region}:${local.account}:function:${local.app_prefix}*"]
  }

  # Site template read — the unfurl handler (/l/*, /s/*) fetches the DEPLOYED
  # index.html out of the web bucket and swaps its og block before serving it.
  # Added 2026-09-07 on Ben's authorization: the app role's own inline grant for
  # this already shipped, but the boundary carried NO s3 statement at all, so the
  # intersection was empty and every GetObject 500'd. No identity-side grant could
  # ever have fixed that.
  #
  # OBJECT-LEVEL ON PURPOSE — not "${local.app_prefix}-site/*". A boundary is the
  # CEILING, not a grant: widening it to the whole bucket gives the lambda nothing
  # today and makes every FUTURE inline grant on this bucket effective without
  # another review. The narrow ARN costs one boundary edit next feature; the wide
  # one costs the review itself. It also matches the app role's shipped inline
  # policy (module.site.s3_bucket_arn/index.html) — a boundary wider than the
  # grant it caps has stopped describing the intent.
  statement {
    sid       = "SiteTemplateRead"
    effect    = "Allow"
    actions   = ["s3:GetObject"]
    resources = ["arn:aws:s3:::${local.app_prefix}-site/index.html"]
  }
}
