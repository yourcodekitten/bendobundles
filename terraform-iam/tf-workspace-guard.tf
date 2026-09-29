# ═══════════════════════════════════════════════════════════════════════════════
# WORKSPACE GUARD — refuse to plan or apply in the DEFAULT workspace.
#
# WHY, measured 2026-09-29 during the tfstate native-locking patrol: this stack's
# backend `key` is BARE (`terraform.tfstate`, see backend.hcl.example), and
# `workspace_key_prefix` ("bendobundles-iam") is applied by the S3 backend ONLY to
# NON-DEFAULT workspaces. So in the default workspace the effective state path is
# not "bendobundles-iam/<ws>/terraform.tfstate" — it is the BUCKET ROOT.
#
# The bucket root is not empty. `brd-prod-ue1-tfstate-store/terraform.tfstate` is
# a 9,189-byte state last written 2024-02-21, predating every repo in this org:
# the account bootstrap. Terraform would ADOPT it as this stack's state and plan
# this stack against resources it does not manage.
#
# Reachable in three commands from a clean checkout:
#   cp backend.hcl.example backend.hcl && terraform init && terraform apply
# with no `terraform workspace new`. This stack's real workspace is "production-iam".
#
# ⚠️ THIS GUARD STOPS THE APPLY, NOT THE LOCK. Terraform acquires the state lock
# before evaluating preconditions, so a default-workspace run still writes
# `terraform.tfstate.tflock` at the bucket ROOT and then deletes it on exit — and
# the deploy identity is prefix-scoped, so that DELETE can 403 and strand a lock on
# the bootstrap state. The guard makes the failure loud and harmless to resources;
# it does not make it free.
#
# ⚠️ THE ROOT CAUSE IS THE BARE `key` AND IS NOT FIXABLE HERE. Path-qualifying it
# would move live state ("bendobundles-iam/production-iam/terraform.tfstate" -> a new path). That is a
# state migration and Ben's call, not an edit. Recorded rather than coded around.
#
# 🔴 MEASURED LIMIT — THIS DOES NOT COVER `terraform destroy`. On a fixture whose
# default workspace was seeded with real state BEFORE the guard was armed:
#     terraform plan            -> precondition FIRES (rc=1)      check block FIRES
#     terraform plan -destroy   -> precondition SILENT            check block SILENT
#                                  ("Plan: 0 to add, 0 to change, 1 to destroy")
# Terraform evaluates neither resource preconditions nor `check` assertions during
# DESTROY planning, so both mechanisms are blind on the operation with the largest
# blast radius. Stated rather than coded around: a guard's silence IS its pass state,
# so a mis-scoped guard is invisible — including this one. (OMBB asked the question
# from the docs and could not settle it; this is the fixture answer.)
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
# Mechanism controlled on an isolated fixture before shipping — THREE arms, because
# two would not have caught the denylist defect:
#     default      -> rc=1  REFUSING (the root-adoption case)
#     producton    -> rc=1  REFUSING (a MISTYPED workspace; the old `!= "default"`
#                                     PASSED this and proposed creating the stack)
#     production-iam-> rc=0  plan succeeds
locals {
  # ALLOWLIST, not a denylist. `!= "default"` accepted every other string, so a
  # MISTYPED workspace ("producton") passed, got the prefix applied, resolved to a
  # path that does not exist, and the plan proposed CREATING THE ENTIRE STACK —
  # duplicate infrastructure rather than root adoption, and on an IAM stack that is
  # not the milder failure. (OMBB on #258.)
  # 🔑 And the guard's own remediation text said `workspace new`, which is the route
  #    to the case it missed. An allowlist closes the hole AND the advice.
  allowed_workspaces = ["production-iam"]
}

resource "terraform_data" "workspace_guard" {
  lifecycle {
    precondition {
      condition     = contains(local.allowed_workspaces, terraform.workspace)
      error_message = <<-EOT
        REFUSING: workspace "${terraform.workspace}" is not an allowed workspace
        for this stack. Allowed: ${join(", ", local.allowed_workspaces)}.

        The backend key is bare and workspace_key_prefix applies only to
        non-default workspaces, so the default workspace resolves to the
        BUCKET ROOT — which holds the account bootstrap state (2024-02-21).
        Continuing would plan this stack against resources it does not manage.

        Fix: terraform workspace select production-iam   (or: workspace new production-iam)
        Then re-run. Run `terraform workspace show` before any apply.
      EOT
    }
  }
}
