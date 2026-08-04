# Regenerated fresh for the lost-months deploy (2026-07-31, gitignored). Secrets (admin_password_hash,
# discord_webhook_url) are passed via TF_VAR env, never written here.
#
# The webhook ENABLED flag, by contrast, is a boolean — not a secret — and lives HERE in
# source (#88 hardening, lilith): the SecureString param is count-gated on it, so leaving it
# to a TF_VAR the operator must remember means a forgotten flag counts a LIVE param to zero
# and destroys it (the #15 near-miss). In source, a `0 to destroy` is impossible to reach by
# forgetting; the plan-gate becomes belt-and-suspenders instead of the only control.
discord_webhook_enabled         = true
aws_account_id                  = "672812236571"
domain_zone_id                  = "Z05311872JYVFOPFTIVOS"
ops_alarm_email                 = "craftsman@bendoerr.me"
lambda_permissions_boundary_arn = "arn:aws:iam::672812236571:policy/brd-prod-ue1-bendobundles-iam-kitten-app-boundary"
humble_username                 = "craftsman@bendoerr.me"
