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
}
