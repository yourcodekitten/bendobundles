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
  # Advanced tier lifts the 4 KB Standard cap: the paste flow PutParameters
  # the raw humble cookie, and an oversized value would ValidationException
  # at runtime. Costs $0.05/mo; overwrite keeps the tier.
  tier = "Advanced"
  tags = module.label_param.tags

  lifecycle {
    ignore_changes = [value]
  }
}

# Humble secure-area step-up secrets — the account password and the app-TOTP
# base32 seed. Humble gates key reveal/redeem/gift behind a fresh-password
# re-auth that the session cookie alone can't pass; fulfillment reads these to
# POST /processlogin and elevate the session before a gift redeem.
#
# Value ownership mirrors humble_cookie: terraform owns the CONTAINER, the value
# is set OUT OF BAND (ben's terminal / kitten-deploy PutParameter) and NEVER in
# code or state-by-us. Both are pre-existing (created out of band on 2026-07-04),
# so the import blocks below adopt them on the first apply; ignore_changes keeps
# the real secret from being reset to the placeholder. Standard tier: both values
# are well under 4 KB.
resource "aws_ssm_parameter" "humble_password" {
  name  = "/${module.label_param.id}/humble-password"
  type  = "SecureString"
  value = "UNSET"
  tags  = module.label_param.tags

  lifecycle {
    ignore_changes = [value]
  }
}

resource "aws_ssm_parameter" "humble_totp_secret" {
  name  = "/${module.label_param.id}/humble-totp-secret"
  type  = "SecureString"
  value = "UNSET"
  tags  = module.label_param.tags

  lifecycle {
    ignore_changes = [value]
  }
}

# Adopt the out-of-band params into state on the first apply. Idempotent — a
# no-op once each is in state, safe to leave in place (prune in a later cleanup).
import {
  to = aws_ssm_parameter.humble_password
  id = "/${module.label_param.id}/humble-password"
}

import {
  to = aws_ssm_parameter.humble_totp_secret
  id = "/${module.label_param.id}/humble-totp-secret"
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

  # Secure-area step-up is opt-in via humble_username: null → the whole feature is
  # off (no env vars, no extra SSM grant) and a gated redeem parks as before.
  humble_step_up_env = var.humble_username == null ? {} : {
    HUMBLE_USERNAME       = var.humble_username
    HUMBLE_PASSWORD_PARAM = aws_ssm_parameter.humble_password.name
    HUMBLE_TOTP_PARAM     = aws_ssm_parameter.humble_totp_secret.name
  }
  humble_step_up_param_arns = var.humble_username == null ? [] : [
    aws_ssm_parameter.humble_password.arn,
    aws_ssm_parameter.humble_totp_secret.arn,
  ]
}
