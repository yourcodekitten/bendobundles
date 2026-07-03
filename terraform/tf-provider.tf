provider "aws" {
  allowed_account_ids = [var.aws_account_id]
  region              = var.region
}

# Route53 zone may live outside this account (org pattern) — pass-through alias.
# If the zone is in the same account, leave route53_profile null.
provider "aws" {
  alias               = "route53"
  allowed_account_ids = var.route53_profile == null ? [var.aws_account_id] : null
  region              = var.region
  profile             = var.route53_profile
}
