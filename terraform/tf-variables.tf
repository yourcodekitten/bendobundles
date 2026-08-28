variable "aws_account_id" {
  type        = string
  description = "Account this stack deploys into (guard against wrong-profile applies)."
}

variable "region" {
  type        = string
  default     = "us-east-1"
  description = "Sole region. CloudFront ACM requires us-east-1; everything colocates."
}

variable "namespace" {
  type        = string
  default     = "brd"
  description = "Org namespace for context/labels."
}

variable "role" {
  type        = string
  default     = "production"
  description = "Context role."
}

variable "domain_zone_name" {
  type        = string
  default     = "bendobundles.com"
  description = "Route53 zone serving the site."
}

variable "domain_zone_id" {
  type        = string
  description = "Route53 hosted zone ID for domain_zone_name."
}

variable "route53_profile" {
  type        = string
  default     = null
  description = "AWS profile for the account holding the Route53 zone, if different."
}

variable "admin_password_hash" {
  type        = string
  sensitive   = true
  description = "Argon2 PHC string for the admin password (generate: `echo -n 'pw' | argon2 \"$(openssl rand -base64 16)\" -id -e`). Stored as SSM SecureString; admin-api refuses to boot without it."
}

variable "discord_webhook_enabled" {
  type        = bool
  default     = false
  description = "Create the SecureString container for the cookie-death Discord webhook. The URL itself is set out of band (PutParameter) — it never passes through terraform. False: no param, no env var, no grant; fulfillment runs webhooks-off."
}

variable "humble_username" {
  type        = string
  default     = null
  description = "Humble account username (email) for secure-area step-up. Null disables step-up (a gated gift redeem parks). Not a secret — the password + TOTP seed live in SSM SecureStrings."
}

variable "lambda_permissions_boundary_arn" {
  type        = string
  default     = null
  description = "ARN of the IAM permissions boundary to set on all three lambda execution roles. Set to the kitten-app-boundary policy ARN so the least-privilege deploy role can safely manage the roles' inline policies (a boundary caps effective permissions regardless of attached policy). Null leaves the roles unbounded. Passed as a variable — the deploy role is explicitly denied GetPolicy on the kitten-* boundary, so it cannot be looked up via a data source."
}

variable "sync_schedule_expression" {
  type        = string
  default     = "cron(0 9 * * ? *)" # 09:00 UTC daily = pre-dawn US-East
  description = "EventBridge schedule for the daily humble sync."
}

variable "ops_alarm_email" {
  type        = string
  description = "Email endpoint for the ops-alarm SNS topic."
}

variable "whisper_enabled" {
  type        = bool
  default     = false # flipped in production.tfvars; default-off so plan-only environments stay silent
  description = "The attic whispers: weekly forgotten-treasure nudge (spec: docs/spec-attic-whispers.md). Creates the schedule, the whisper webhook SSM container, and the never-ran alarm."
}

variable "whisper_schedule_expression" {
  type        = string
  default     = "cron(0 10 ? * SAT,SUN *)"
  description = "Whisper cadence in America/New_York. The SUNDAY tick is a HEARTBEAT, not a second whisper: Saturday always wins the ISO-week slot, Sunday exits as the designed conditional-put loser — it keeps InvocationAttemptCount present at ≤6-day gaps (the never-ran alarm lives under AWS's hard 7-day evaluation cap, measured 2026-08-28 when the API refused 8 daily buckets), and doubles as the retry day if a Saturday tick outright fails. Cadence in America/New_York (EventBridge Scheduler is timezone-aware; classic rules are UTC-only and drift an hour across DST, which is why this rides aws_scheduler_schedule). ⚠️ The whisper-log slot key is the ISO WEEK — grain coupled to a weekly cadence; a sub-weekly schedule must change the slot derivation in fulfillment in the same commit."
}
