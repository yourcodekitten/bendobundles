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
# Gated on humble_username (same shape as discord_webhook): with the feature off,
# NOTHING is created and nothing sits in state. Value ownership mirrors
# humble_cookie: terraform owns the CONTAINER, the value is set OUT OF BAND
# (kitten-deploy PutParameter) and NEVER in code or state-by-us; ignore_changes
# keeps the real secret from being reset to the placeholder. Standard tier.
#
# NO import {} blocks: an unconditional import hard-fails `terraform plan`
# ("Cannot import non-existent remote object") in any account where the params
# weren't hand-created — fresh env, second region, DR rebuild — because it
# preempts the create path, and CI only runs `validate` so the break is invisible
# until a live plan. Where the params already exist out of band (prod, created
# 2026-07-04), adopt them with a ONE-TIME CLI import during deploy:
#   terraform import 'aws_ssm_parameter.humble_password[0]'    /<param-prefix>/humble-password
#   terraform import 'aws_ssm_parameter.humble_totp_secret[0]' /<param-prefix>/humble-totp-secret
# A fresh account with humble_username set just creates them at "UNSET".
resource "aws_ssm_parameter" "humble_password" {
  count = var.humble_username == null ? 0 : 1
  name  = "/${module.label_param.id}/humble-password"
  type  = "SecureString"
  value = "UNSET"
  tags  = module.label_param.tags

  lifecycle {
    ignore_changes = [value]
  }
}

resource "aws_ssm_parameter" "humble_totp_secret" {
  count = var.humble_username == null ? 0 : 1
  name  = "/${module.label_param.id}/humble-totp-secret"
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

  # Secure-area step-up is opt-in via humble_username: null → the whole feature is
  # off (no params, no env vars, no extra SSM grant) and a gated redeem parks as
  # before. Non-null → the count-gated params exist at [0].
  humble_step_up_env = var.humble_username == null ? {} : {
    HUMBLE_USERNAME       = var.humble_username
    HUMBLE_PASSWORD_PARAM = aws_ssm_parameter.humble_password[0].name
    HUMBLE_TOTP_PARAM     = aws_ssm_parameter.humble_totp_secret[0].name
  }
  humble_step_up_param_arns = var.humble_username == null ? [] : [
    aws_ssm_parameter.humble_password[0].arn,
    aws_ssm_parameter.humble_totp_secret[0].arn,
  ]
}
