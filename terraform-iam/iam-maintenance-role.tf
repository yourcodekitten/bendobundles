# ── kitten-maintenance role (ITEM DATA-PLANE, run-once jobs) ──────────────────
# The identity for operator-run maintenance bins (backfill_details and friends):
# item-level reads and writes on the app tables, and nothing else. Exists so the
# backfill never again depends on a console hand-edit (the 2026-07-08 temp grant
# on kitten-debug that a later apply silently reverted — see #59, #71), and so
# kitten-deploy stays pure control-plane: a bad terraform run and a bad data
# mangling should never share a credential.
module "label_maintenance" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "kitten-maintenance"
}

resource "aws_iam_role" "kitten_maintenance" {
  name                 = module.label_maintenance.id
  assume_role_policy   = data.aws_iam_policy_document.trust_manager.json
  max_session_duration = 3600
  tags                 = module.label_maintenance.tags
}

data "aws_iam_policy_document" "maintenance" {
  # Item data-plane on the app tables + indexes, what the backfill needs:
  # Scan the catalog, read-modify-write STEAMAPP# items, and the auto-hide
  # conditional full-item PutItem on GAME# items (mirrors set_game_hidden).
  # UpdateItem rides along for future maintenance bins. Deliberately NO
  # DeleteItem — maintenance rewrites items, it never removes them — and no
  # control-plane, no other services.
  statement {
    sid    = "DynamoDbItemMaintenance"
    effect = "Allow"
    actions = [
      "dynamodb:GetItem",
      "dynamodb:BatchGetItem",
      "dynamodb:Query",
      "dynamodb:Scan",
      "dynamodb:PutItem",
      "dynamodb:UpdateItem",
    ]
    resources = [
      "arn:aws:dynamodb:${local.region}:${local.account}:table/${local.app_prefix}*",
      "arn:aws:dynamodb:${local.region}:${local.account}:table/${local.app_prefix}*/index/*",
    ]
  }

  # Same hard boundary as kitten-debug: even if the Allow above ever widens,
  # this role can never read SecureString values or decrypt app secrets.
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

resource "aws_iam_role_policy" "maintenance" {
  name   = "dynamodb-item-maintenance"
  role   = aws_iam_role.kitten_maintenance.id
  policy = data.aws_iam_policy_document.maintenance.json
}
