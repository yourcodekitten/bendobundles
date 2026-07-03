module "context" {
  source    = "bendoerr-terraform-modules/context/null"
  version   = "0.5.2"
  namespace = var.namespace
  role      = var.role
  region    = var.region
  project   = "bendobundles"
}
