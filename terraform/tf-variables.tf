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
  default     = "bd"
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

variable "discord_webhook_url" {
  type        = string
  default     = null
  sensitive   = true
  description = "Optional Discord webhook for cookie-death pings. Null disables (fulfillment treats a missing param as webhooks-off)."
}

variable "sync_schedule_expression" {
  type        = string
  default     = "cron(0 9 * * ? *)" # 09:00 UTC daily = pre-dawn US-East
  description = "EventBridge schedule for the daily humble sync."
}
