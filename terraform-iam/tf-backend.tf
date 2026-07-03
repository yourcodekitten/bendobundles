# Partial backend — separate state from the main stack (so the deploy role,
# which manages the MAIN stack's state, never touches THIS stack's state).
#   terraform init -backend-config=backend.hcl
terraform {
  backend "s3" {}
}
