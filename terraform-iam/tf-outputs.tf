output "kitten_manager_access_key_id" {
  value       = aws_iam_access_key.kitten_manager.id
  description = "Access key ID for the kitten-manager user."
}

output "kitten_manager_secret_access_key" {
  value       = aws_iam_access_key.kitten_manager.secret
  sensitive   = true
  description = "Secret. Read once with `terraform output -raw kitten_manager_secret_access_key` and hand to kitten. Also lives in the (encrypted) state."
}

output "kitten_debug_role_arn" {
  value       = aws_iam_role.kitten_debug.arn
  description = "Assume this for read-only diagnostics."
}

output "kitten_deploy_role_arn" {
  value       = aws_iam_role.kitten_deploy.arn
  description = "Assume this to run terraform / deploy-web on the main stack."
}

output "kitten_maintenance_role_arn" {
  value       = aws_iam_role.kitten_maintenance.arn
  description = "Assume this to run operator maintenance bins (backfill_details) — item data-plane only."
}
