# ── kitten-manager user ───────────────────────────────────────────────────────
# The identity whose long-lived access key is handed to kitten. It has NO direct
# permissions of its own — its ONLY power is assuming the two roles below. So a
# leaked key can do nothing but assume a role, and every assumed action is
# short-lived + attributable in CloudTrail (role session name).
module "label_manager" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "kitten-mgr"
}

resource "aws_iam_user" "kitten_manager" {
  name = module.label_manager.id
  tags = module.label_manager.tags
}

resource "aws_iam_access_key" "kitten_manager" {
  user = aws_iam_user.kitten_manager.name
}

# The user's SOLE permission: assume the debug and deploy roles. Nothing else.
data "aws_iam_policy_document" "manager_assume" {
  statement {
    sid       = "AssumeKittenRoles"
    effect    = "Allow"
    actions   = ["sts:AssumeRole"]
    resources = [aws_iam_role.kitten_debug.arn, aws_iam_role.kitten_deploy.arn]
  }
}

resource "aws_iam_user_policy" "manager_assume" {
  name   = "assume-kitten-roles"
  user   = aws_iam_user.kitten_manager.name
  policy = data.aws_iam_policy_document.manager_assume.json
}

# Both roles trust exactly this user to assume them (no account-wide trust).
data "aws_iam_policy_document" "trust_manager" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole"]
    principals {
      type        = "AWS"
      identifiers = [aws_iam_user.kitten_manager.arn]
    }
  }
}
