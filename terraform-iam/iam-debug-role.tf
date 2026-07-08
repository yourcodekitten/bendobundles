# ── kitten-debug role (READ-ONLY + one scoped write) ──────────────────────────
# Everything needed to diagnose the live stack — tail logs, read Lambda config,
# inspect DynamoDB item/claim state, look at API Gateway / CloudFront / S3 /
# EventBridge — and NOTHING that mutates or decrypts app secrets, with a single
# deliberate carve-out: PutItem confined to STEAMAPP#* cache items, so the
# fulfillment backfill bin can rebuild the Steam cache (see SteamCacheWrite).
module "label_debug" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "kitten-debug"
}

resource "aws_iam_role" "kitten_debug" {
  name                 = module.label_debug.id
  assume_role_policy   = data.aws_iam_policy_document.trust_manager.json
  max_session_duration = 3600
  tags                 = module.label_debug.tags
}

data "aws_iam_policy_document" "debug" {
  # Read-only observability + state inspection. Most read/list/describe actions
  # do not support resource-level scoping, hence resources = ["*"]; the Deny
  # statement below is what actually bounds this role.
  statement {
    sid    = "ReadOnlyDiagnostics"
    effect = "Allow"
    actions = [
      # CloudWatch Logs — the primary debugging surface (tail / filter).
      "logs:GetLogEvents",
      "logs:FilterLogEvents",
      "logs:DescribeLogGroups",
      "logs:DescribeLogStreams",
      "logs:StartLiveTail",
      "logs:StopLiveTail",
      "logs:GetQueryResults",
      "logs:StartQuery",
      "logs:StopQuery",
      # CloudWatch metrics.
      "cloudwatch:GetMetricData",
      "cloudwatch:GetMetricStatistics",
      "cloudwatch:ListMetrics",
      # Lambda config / policy (not code download, not invoke).
      "lambda:GetFunction",
      "lambda:GetFunctionConfiguration",
      "lambda:GetPolicy",
      "lambda:ListFunctions",
      "lambda:ListEventSourceMappings",
      # DynamoDB reads — inspect claim / game / link / sync state.
      "dynamodb:GetItem",
      "dynamodb:BatchGetItem",
      "dynamodb:Query",
      "dynamodb:Scan",
      "dynamodb:DescribeTable",
      "dynamodb:DescribeTimeToLive",
      # API Gateway (GET covers all read operations on REST APIs).
      "apigateway:GET",
      # CloudFront.
      "cloudfront:GetDistribution",
      "cloudfront:GetDistributionConfig",
      "cloudfront:ListDistributions",
      "cloudfront:GetFunction",
      "cloudfront:DescribeFunction",
      # EventBridge.
      "events:DescribeRule",
      "events:ListRules",
      "events:ListTargetsByRule",
      # SSM metadata ONLY — describe parameters + history WITHOUT their values.
      "ssm:DescribeParameters",
      # IAM read — inspect the lambda roles' policies.
      "iam:GetRole",
      "iam:GetRolePolicy",
      "iam:ListRolePolicies",
      "iam:ListAttachedRolePolicies",
      "iam:GetPolicy",
      "iam:GetPolicyVersion",
      # Who am I.
      "sts:GetCallerIdentity",
    ]
    resources = ["*"]
  }

  # S3 — the SPA bucket only. These actions DO support resource scoping (unlike
  # most of the read/list/describe block above), so they get their own statement:
  # account-wide s3 read would be breadth without purpose, and the grant should
  # match what it's for.
  statement {
    sid    = "S3SpaRead"
    effect = "Allow"
    actions = [
      "s3:ListBucket",
      "s3:GetObject",
      "s3:GetBucketPolicy",
      "s3:GetBucketLocation",
    ]
    resources = [
      "arn:aws:s3:::${local.app_prefix}*",
      "arn:aws:s3:::${local.app_prefix}*/*",
    ]
  }

  # The ONE write this role can perform: rewrite Steam appdetails cache items.
  # Exists for the run-once fulfillment `backfill_genres` bin (issue #57) and
  # future cache rebuilds after a parse change — the bin's only write is
  # put_steam_app → PutItem. The LeadingKeys condition confines writes to
  # partition keys beginning STEAMAPP#, so games, links, claims, and sync state
  # stay untouchable; item reads/deletes and every other mutation remain denied
  # by omission.
  statement {
    sid     = "SteamCacheWrite"
    effect  = "Allow"
    actions = ["dynamodb:PutItem"]
    resources = [
      "arn:aws:dynamodb:${local.region}:${local.account}:table/${local.app_prefix}*",
    ]
    condition {
      test     = "ForAllValues:StringLike"
      variable = "dynamodb:LeadingKeys"
      values   = ["STEAMAPP#*"]
    }
  }

  # HARD BOUNDARY: the debug role can never read the humble session cookie or
  # the admin password hash. Deny reading SecureString VALUES and any KMS
  # decrypt — even holding these credentials, kitten cannot see ben's secrets.
  # (Debugging needs parameter metadata + histories, never the plaintext.)
  statement {
    sid    = "NeverDecryptAppSecrets"
    effect = "Deny"
    actions = [
      "ssm:GetParameter",
      "ssm:GetParameters",
      "ssm:GetParametersByPath",
      "kms:Decrypt",
      "kms:GenerateDataKey",
    ]
    resources = ["*"]
  }
}

resource "aws_iam_role_policy" "debug" {
  name   = "diagnostics-read-only"
  role   = aws_iam_role.kitten_debug.id
  policy = data.aws_iam_policy_document.debug.json
}
