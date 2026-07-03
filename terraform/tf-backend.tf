# Partial backend — ben supplies values at init time:
#   terraform init -backend-config=backend.hcl
# (copy backend.hcl.example to backend.hcl and adjust; backend.hcl is gitignored)
terraform {
  backend "s3" {}
}
