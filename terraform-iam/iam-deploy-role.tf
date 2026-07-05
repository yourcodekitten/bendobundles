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
      # The provider reads code-signing config as part of every function
      # update — without this the apply dies mid-flight AFTER
      # UpdateFunctionCode succeeds (caught live on the first kitten deploy,
      # 2026-07-04: fulfillment updated, public-api/admin-api stranded).
      "lambda:GetFunctionCodeSigningConfig",
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
  # potential into three statements: reads/tags and deletions ride the name scope
  # (below); setting a boundary is condition-locked to the kitten-app-boundary
  # (IamAppRolesSetBoundary); and policy/trust mutations ride the name scope but
  # are contained by the boundary every app role already carries — a boundary caps
  # effective permissions no matter what policy is attached. (PutRolePolicy cannot
  # be boundary-conditioned: AWS doesn't populate iam:PermissionsBoundary for it.)
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
  # BOUNDARY-SETTING actions: a new app role can only be BORN carrying the
  # kitten-app-boundary, and an existing role's boundary can only ever be SET to
  # it. AWS populates the iam:PermissionsBoundary request key for exactly these
  # two actions, so the condition is enforceable here — this is the statement that
  # guarantees every app role is boundaried.
  statement {
    sid    = "IamAppRolesSetBoundary"
    effect = "Allow"
    actions = [
      "iam:CreateRole",
      "iam:PutRolePermissionsBoundary",
    ]
    resources = ["arn:aws:iam::${local.account}:role/${local.app_prefix}*"]
    condition {
      test     = "StringEquals"
      variable = "iam:PermissionsBoundary"
      values   = [aws_iam_policy.app_boundary.arn]
    }
  }
  # POLICY-MUTATION actions: update an existing app role's inline/attached policies
  # or trust. These CANNOT carry the iam:PermissionsBoundary condition — AWS does
  # not populate that key for them, so bundling them with CreateRole (as this once
  # did) silently DENIED all three: no deploy could update an app role's policy
  # (caught live on the 2026-07-04 fulfillment deploy — iam:PutRolePolicy 403).
  # Containment does NOT rest on this condition; it rests on the boundary the roles
  # already carry (set via IamAppRolesSetBoundary at create time / migration): a
  # permissions boundary caps a role's EFFECTIVE permissions no matter what policy
  # is attached, so a wider inline policy cannot escalate past the ceiling. Resource
  # scope (app_prefix*) + the NeverTouchKittenIam Deny floor still exclude this
  # stack's own identities. (Prereq: all app roles boundaried — done 2026-07-04.)
  statement {
    sid    = "IamAppRolesMutatePolicies"
    effect = "Allow"
    actions = [
      "iam:PutRolePolicy",
      "iam:AttachRolePolicy",
      "iam:UpdateAssumeRolePolicy",
    ]
    resources = ["arn:aws:iam::${local.account}:role/${local.app_prefix}*"]
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
      "ssm:AddTagsToResource",
      "ssm:RemoveTagsFromResource",
      "ssm:ListTagsForResource",
    ]
    resources = ["arn:aws:ssm:${local.region}:${local.account}:parameter/${local.app_prefix}*"]
  }
  # DescribeParameters is a list-type action AWS evaluates against * — a
  # parameter-ARN scope NEVER matches, so keeping it in the scoped statement
  # above is a silent no-grant (caught live on the first kitten deploy,
  # 2026-07-04: terraform refresh of every managed param 403'd). It exposes
  # parameter METADATA only; values stay behind the scoped GetParameter.
  # Resource-scoping is off the table but condition-scoping is not: the
  # region condition keeps enumeration to the one region the live failure
  # demonstrated a need for, instead of a whole-account targeting map.
  statement {
    sid       = "SsmDescribeUnscopeable"
    effect    = "Allow"
    actions   = ["ssm:DescribeParameters"]
    resources = ["*"]

    condition {
      test     = "StringEquals"
      variable = "aws:RequestedRegion"
      values   = [local.region]
    }
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
  # The API Gateway ACCESS-log group is named by the apigateway module as
  # /aws/apigateway/<label>-access-logs — a path NONE of the three patterns
  # above match, so refreshing it 403'd on logs:ListTagsForResource in real
  # deploys and forced -refresh=false (which masked a partial-apply drift
  # once). READ-ONLY on purpose: this PR is the read-gap pass. The same
  # pattern-miss also blocks WRITES to that group (retention changes / delete
  # through terraform will still 403) — that is a separate follow-up decision,
  # not something to smuggle into a read fix. Legacy ListTagsLogGroup rides
  # along to mirror the new/old tag-API pairing the statement above uses.
  statement {
    sid    = "LogsReadApigwAccessLogs"
    effect = "Allow"
    actions = [
      "logs:ListTagsForResource",
      "logs:ListTagsLogGroup",
    ]
    resources = [
      "arn:aws:logs:${local.region}:${local.account}:log-group:/aws/apigateway/${local.app_prefix}*",
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
      # The site module's data.aws_cloudfront_cache_policy lookup (by name,
      # "Managed-CachingOptimized") runs at EVERY plan: the provider pages
      # ListCachePolicies to resolve the id, then GetCachePolicy to read it.
      # Without both, the refresh 403s and plans only survive under
      # -refresh=false — the flag that masked a real partial-apply drift once.
      # ListCachePolicies is a list-type action evaluated against * (like
      # ssm:DescribeParameters above); this statement is already resources=["*"].
      # Both are pure reads of AWS-managed policy definitions, no secret values.
      "cloudfront:ListCachePolicies",
      "cloudfront:GetCachePolicy",
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
