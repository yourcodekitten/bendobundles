# ── kitten-deploy role (POWERFUL — review this one hardest) ────────────────────
# Everything needed to `terraform apply` the MAIN stack and run deploy-web.sh
# (S3 sync + CloudFront invalidation). "Can run terraform for this stack" is
# inherently close to admin-over-these-services; the policy is scoped by
# service (modeled on the org's tfuser apply-role toggles) and by resource
# where the service supports it. Assumed DELIBERATELY, only for deploys — routine
# debugging uses kitten-debug, which cannot touch any of this or read secrets.
module "label_deploy" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "kitten-deploy"
}

resource "aws_iam_role" "kitten_deploy" {
  name                 = module.label_deploy.id
  assume_role_policy   = data.aws_iam_policy_document.trust_manager.json
  max_session_duration = 3600
  tags                 = module.label_deploy.tags
}

data "aws_iam_policy_document" "deploy" {
  # ── Terraform state backend — MAIN stack prefix ONLY ────────────────────────
  # Scoped to the main stack's state, NOT this IAM stack's. State scoping is one
  # of three containment layers; the other two live in the IAM section below
  # (permissions boundary on grants + the NeverTouchKittenIam Deny floor).
  statement {
    sid       = "StateBucketList"
    effect    = "Allow"
    actions   = ["s3:ListBucket"]
    resources = ["arn:aws:s3:::${var.state_bucket}"]
    condition {
      test     = "StringLike"
      variable = "s3:prefix"
      values   = ["${var.main_stack_state_prefix}/*"]
    }
  }
  statement {
    sid       = "StateObjectRW"
    effect    = "Allow"
    actions   = ["s3:GetObject", "s3:PutObject", "s3:DeleteObject"]
    resources = ["arn:aws:s3:::${var.state_bucket}/${var.main_stack_state_prefix}/*"]
  }

  # ── Lambda — list on * (unscopeable), everything else on app functions ──────
  statement {
    sid       = "LambdaList"
    effect    = "Allow"
    actions   = ["lambda:ListFunctions"]
    resources = ["*"]
  }
  statement {
    sid    = "Lambda"
    effect = "Allow"
    actions = [
      "lambda:CreateFunction",
      "lambda:DeleteFunction",
      "lambda:UpdateFunctionCode",
      "lambda:UpdateFunctionConfiguration",
      "lambda:AddPermission",
      "lambda:RemovePermission",
      "lambda:GetFunction",
      "lambda:GetFunctionConfiguration",
      "lambda:GetPolicy",
      "lambda:ListVersionsByFunction",
      "lambda:TagResource",
      "lambda:UntagResource",
      "lambda:PublishVersion",
    ]
    resources = ["arn:aws:lambda:${local.region}:${local.account}:function:${local.app_prefix}*"]
  }

  # ── IAM — manage the app's roles, CONTAINED by the permissions boundary ─────
  # Naming-scope alone does not contain this: PutRolePolicy on an app execution
  # role + UpdateFunctionCode + invoke = account admin. Split by escalation
  # potential — reads and deletions ride the name scope; anything that GRANTS
  # power additionally requires the target role to carry the kitten-app-boundary
  # (created WITH it, for CreateRole), so effective permissions can never exceed
  # the boundary no matter what policy gets attached.
  statement {
    sid    = "IamAppRolesRead"
    effect = "Allow"
    actions = [
      "iam:GetRole",
      "iam:GetRolePolicy",
      "iam:ListRolePolicies",
      "iam:ListAttachedRolePolicies",
      "iam:TagRole",
      "iam:UntagRole",
    ]
    resources = ["arn:aws:iam::${local.account}:role/${local.app_prefix}*"]
  }
  statement {
    sid    = "IamAppRolesShrink"
    effect = "Allow"
    actions = [
      "iam:DeleteRole",
      "iam:DeleteRolePolicy",
      "iam:DetachRolePolicy",
    ]
    resources = ["arn:aws:iam::${local.account}:role/${local.app_prefix}*"]
  }
  statement {
    sid    = "IamAppRolesGrowBounded"
    effect = "Allow"
    actions = [
      "iam:CreateRole",
      "iam:PutRolePolicy",
      "iam:AttachRolePolicy",
      "iam:UpdateAssumeRolePolicy",
      # For migrating the already-deployed app roles under the boundary.
      "iam:PutRolePermissionsBoundary",
    ]
    resources = ["arn:aws:iam::${local.account}:role/${local.app_prefix}*"]
    condition {
      test     = "StringEquals"
      variable = "iam:PermissionsBoundary"
      values   = [aws_iam_policy.app_boundary.arn]
    }
  }
  # PassRole only into the services this stack actually hands roles to.
  statement {
    sid       = "IamPassRoleToServices"
    effect    = "Allow"
    actions   = ["iam:PassRole"]
    resources = ["arn:aws:iam::${local.account}:role/${local.app_prefix}*"]
    condition {
      test     = "StringEquals"
      variable = "iam:PassedToService"
      values   = ["lambda.amazonaws.com", "apigateway.amazonaws.com"]
    }
  }
  # HARD FLOOR: never any IAM action on this stack's own identities or ceiling.
  # Needed because label naming puts these at brd-…-bendobundles-iam-kitten-*,
  # which the ${app_prefix}* glob above MATCHES — without this Deny the Allows
  # would cover the deploy role itself (self-modification path).
  statement {
    sid     = "NeverTouchKittenIam"
    effect  = "Deny"
    actions = ["iam:*"]
    resources = [
      "arn:aws:iam::${local.account}:role/*kitten*",
      "arn:aws:iam::${local.account}:user/*kitten*",
      "arn:aws:iam::${local.account}:policy/*kitten*",
    ]
  }

  # ── DynamoDB — the app table + indexes ──────────────────────────────────────
  statement {
    sid    = "DynamoDb"
    effect = "Allow"
    actions = [
      "dynamodb:CreateTable",
      "dynamodb:DeleteTable",
      "dynamodb:DescribeTable",
      "dynamodb:DescribeContinuousBackups",
      "dynamodb:UpdateContinuousBackups",
      "dynamodb:DescribeTimeToLive",
      "dynamodb:UpdateTimeToLive",
      "dynamodb:UpdateTable",
      "dynamodb:TagResource",
      "dynamodb:UntagResource",
      "dynamodb:ListTagsOfResource",
    ]
    resources = [
      "arn:aws:dynamodb:${local.region}:${local.account}:table/${local.app_prefix}*",
    ]
  }

  # ── SSM parameters (app namespace) ──────────────────────────────────────────
  # PutParameter/DeleteParameter + GetParameter: terraform reads current values
  # on refresh (incl. SecureStrings it manages), so the deploy role CAN decrypt
  # the app secrets. This is inherent to "terraform owns these params" — it's
  # why routine work uses the debug role (which cannot). Scoped to /app/* params.
  statement {
    sid    = "SsmAppParams"
    effect = "Allow"
    actions = [
      "ssm:PutParameter",
      "ssm:DeleteParameter",
      "ssm:GetParameter",
      "ssm:GetParameters",
      "ssm:DescribeParameters",
      "ssm:AddTagsToResource",
      "ssm:RemoveTagsFromResource",
      "ssm:ListTagsForResource",
    ]
    resources = ["arn:aws:ssm:${local.region}:${local.account}:parameter/${local.app_prefix}*"]
  }

  # ── EventBridge (the daily-sync rule) ───────────────────────────────────────
  statement {
    sid    = "Events"
    effect = "Allow"
    actions = [
      "events:PutRule",
      "events:DeleteRule",
      "events:DescribeRule",
      "events:PutTargets",
      "events:RemoveTargets",
      "events:ListTargetsByRule",
      "events:ListTagsForResource",
      "events:TagResource",
      "events:UntagResource",
    ]
    resources = ["arn:aws:events:${local.region}:${local.account}:rule/${local.app_prefix}*"]
  }

  # ── CloudWatch Logs — describe on * (unscopeable), mutation on the stack's
  # groups: app-prefixed (lambda) + API-Gateway-Execution-Logs_* (named by AWS).
  statement {
    sid       = "LogsDescribe"
    effect    = "Allow"
    actions   = ["logs:DescribeLogGroups"]
    resources = ["*"]
  }
  statement {
    sid    = "Logs"
    effect = "Allow"
    actions = [
      "logs:CreateLogGroup",
      "logs:DeleteLogGroup",
      "logs:PutRetentionPolicy",
      "logs:TagResource",
      "logs:ListTagsForResource",
      "logs:TagLogGroup",
      "logs:ListTagsLogGroup",
    ]
    resources = [
      "arn:aws:logs:${local.region}:${local.account}:log-group:/aws/lambda/${local.app_prefix}*",
      "arn:aws:logs:${local.region}:${local.account}:log-group:${local.app_prefix}*",
      "arn:aws:logs:${local.region}:${local.account}:log-group:API-Gateway-Execution-Logs_*",
    ]
  }

  # ── Route53 — validation + alias records in the site's zone ─────────────────
  statement {
    sid    = "Route53Zone"
    effect = "Allow"
    actions = [
      "route53:ChangeResourceRecordSets",
      "route53:GetHostedZone",
      "route53:ListResourceRecordSets",
    ]
    resources = ["arn:aws:route53:::hostedzone/*"]
  }
  statement {
    sid       = "Route53Read"
    effect    = "Allow"
    actions   = ["route53:GetChange", "route53:ListHostedZones"]
    resources = ["*"]
  }

  # ── Services that don't scope cleanly by resource at create time ────────────
  # apigateway / cloudfront / acm are account-wide within-service (REST APIs and
  # CloudFront distributions get generated ids; ACM certs are created fresh). KMS
  # is needed for the state bucket SSE + the SSM SecureString key. These are the
  # broadest grants — the ones to eyeball.
  statement {
    sid    = "ApiGateway"
    effect = "Allow"
    actions = [
      "apigateway:GET",
      "apigateway:POST",
      "apigateway:PUT",
      "apigateway:PATCH",
      "apigateway:DELETE",
    ]
    resources = ["arn:aws:apigateway:${local.region}::*"]
  }
  statement {
    sid    = "CloudFront"
    effect = "Allow"
    actions = [
      "cloudfront:CreateDistribution",
      "cloudfront:UpdateDistribution",
      "cloudfront:DeleteDistribution",
      "cloudfront:GetDistribution",
      "cloudfront:GetDistributionConfig",
      "cloudfront:ListDistributions",
      "cloudfront:TagResource",
      "cloudfront:UntagResource",
      "cloudfront:ListTagsForResource",
      "cloudfront:CreateInvalidation",
      "cloudfront:GetInvalidation",
      "cloudfront:CreateOriginAccessControl",
      "cloudfront:UpdateOriginAccessControl",
      "cloudfront:DeleteOriginAccessControl",
      "cloudfront:GetOriginAccessControl",
      "cloudfront:ListOriginAccessControls",
      "cloudfront:CreateFunction",
      "cloudfront:UpdateFunction",
      "cloudfront:DeleteFunction",
      "cloudfront:DescribeFunction",
      "cloudfront:GetFunction",
      "cloudfront:PublishFunction",
      "cloudfront:AssociateAlias",
    ]
    resources = ["*"]
  }
  statement {
    sid    = "Acm"
    effect = "Allow"
    actions = [
      "acm:RequestCertificate",
      "acm:DeleteCertificate",
      "acm:DescribeCertificate",
      "acm:ListCertificates",
      "acm:AddTagsToCertificate",
      "acm:ListTagsForCertificate",
      "acm:GetCertificate",
    ]
    resources = ["*"]
  }
  statement {
    sid    = "Kms"
    effect = "Allow"
    actions = [
      "kms:Decrypt",
      "kms:Encrypt",
      "kms:GenerateDataKey",
      "kms:DescribeKey",
    ]
    resources = ["*"]
  }

  # ── deploy-web.sh: publish the SPA to the site bucket ───────────────────────
  statement {
    sid    = "SiteBucket"
    effect = "Allow"
    actions = [
      "s3:CreateBucket",
      "s3:DeleteBucket",
      "s3:ListBucket",
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
      "s3:GetBucketPolicy",
      "s3:PutBucketPolicy",
      "s3:DeleteBucketPolicy",
      "s3:GetBucketPublicAccessBlock",
      "s3:PutBucketPublicAccessBlock",
      "s3:GetBucketTagging",
      "s3:PutBucketTagging",
      "s3:GetBucketVersioning",
      "s3:PutBucketVersioning",
      "s3:GetEncryptionConfiguration",
      "s3:PutEncryptionConfiguration",
      "s3:GetLifecycleConfiguration",
      "s3:PutLifecycleConfiguration",
      "s3:GetBucketAcl",
      "s3:GetBucketCORS",
      "s3:GetBucketWebsite",
      "s3:GetAccelerateConfiguration",
      "s3:GetBucketRequestPayment",
      "s3:GetBucketLogging",
      "s3:GetBucketObjectLockConfiguration",
      "s3:GetReplicationConfiguration",
      "s3:GetBucketNotification",
      "s3:GetBucketOwnershipControls",
      "s3:PutBucketOwnershipControls",
    ]
    resources = [
      "arn:aws:s3:::${local.app_prefix}*",
      "arn:aws:s3:::${local.app_prefix}*/*",
    ]
  }
}

resource "aws_iam_role_policy" "deploy" {
  name   = "terraform-deploy"
  role   = aws_iam_role.kitten_deploy.id
  policy = data.aws_iam_policy_document.deploy.json
}
