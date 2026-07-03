module "context" {
  source    = "bendoerr-terraform-modules/context/null"
  version   = "0.5.2"
  namespace = var.namespace
  role      = var.role
  region    = var.region
  project   = "bendobundles-iam"
}

# Convenience locals for resource-scoped policies. The main stack labels its
# resources brd-<env>-bendobundles-<name>; we grant the deploy role over that
# family plus the shared state bucket.
locals {
  account = var.aws_account_id
  region  = var.region

  # e.g. brd-prod-ue1-bendobundles
  app_prefix = "${module.context.shared.namespace}-${module.context.shared.environment}-bendobundles"
}
