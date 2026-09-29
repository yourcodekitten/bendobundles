# ═══════════════════════════════════════════════════════════════════════════════
# WORKSPACE GUARD — refuse to plan or apply in the DEFAULT workspace.
#
# WHY, measured 2026-09-29 during the tfstate native-locking patrol: this stack's
# backend `key` is BARE (`terraform.tfstate`, see backend.hcl.example), and
# `workspace_key_prefix` ("bendobundles") is applied by the S3 backend ONLY to
# NON-DEFAULT workspaces. So in the default workspace the effective state path is
# not "bendobundles/<ws>/terraform.tfstate" — it is the BUCKET ROOT.
#
# The bucket root is not empty. `brd-prod-ue1-tfstate-store/terraform.tfstate` is
# a 9,189-byte state last written 2024-02-21, predating every repo in this org:
# the account bootstrap. Terraform would ADOPT it as this stack's state and plan
# this stack against resources it does not manage.
#
# Reachable in three commands from a clean checkout:
#   cp backend.hcl.example backend.hcl && terraform init && terraform apply
# with no `terraform workspace new`. This stack's real workspace is "production".
#
# ⚠️ THIS GUARD STOPS THE APPLY, NOT THE LOCK. Terraform acquires the state lock
# before evaluating preconditions, so a default-workspace run still writes
# `terraform.tfstate.tflock` at the bucket ROOT and then deletes it on exit — and
# the deploy identity is prefix-scoped, so that DELETE can 403 and strand a lock on
# the bootstrap state. The guard makes the failure loud and harmless to resources;
# it does not make it free.
#
# ⚠️ THE ROOT CAUSE IS THE BARE `key` AND IS NOT FIXABLE HERE. Path-qualifying it
# would move live state ("bendobundles/production/terraform.tfstate" -> a new path). That is a
# state migration and Ben's call, not an edit. Recorded rather than coded around.
#
# NOT a `check` block: those only WARN, and the consequence here is a plan against
# the account foundation. This must refuse.
#
# It costs one no-op entry in state, DELIBERATELY. A precondition hung on some
# unrelated existing resource would vanish the day that resource is refactored away
# — and a guard whose quiet state is its pass state is the defect this repo has been
# paying for all day.
#
# CI-safe, measured not assumed: ci.yml runs `init -backend=false` + `validate`,
# and preconditions are evaluated at PLAN time, not by `validate`. Verified green.
# Mechanism red/green-controlled before shipping: default workspace -> rc=1 naming
# the refusal; workspace "production" -> rc=0, plan succeeds.
resource "terraform_data" "workspace_guard" {
  lifecycle {
    precondition {
      condition     = terraform.workspace != "default"
      error_message = <<-EOT
        REFUSING: this stack is running in the DEFAULT workspace.

        The backend key is bare and workspace_key_prefix applies only to
        non-default workspaces, so the default workspace resolves to the
        BUCKET ROOT — which holds the account bootstrap state (2024-02-21).
        Continuing would plan this stack against resources it does not manage.

        Fix: terraform workspace select production   (or: workspace new production)
        Then re-run. Run `terraform workspace show` before any apply.
      EOT
    }
  }
}
