output "dynamodb_table_name" {
  value = aws_dynamodb_table.this.name
}

# Handy for ops: `aws logs tail /aws/lambda/<name>`, manual invokes, etc.
output "lambda_function_names" {
  value = {
    public_api  = module.lambda_public_api.lambda_function_name
    admin_api   = module.lambda_admin_api.lambda_function_name
    fulfillment = module.lambda_fulfillment.lambda_function_name
  }
}

output "api_stage_invoke_url" {
  value = module.apigateway.stage_invoke_url
}

output "site_url" {
  value = module.site.cloudfront_distribution_alias_domain_name
}

output "s3_bucket_id" {
  value = module.site.s3_bucket_id
}

output "cloudfront_distribution_id" {
  value = module.site.cloudfront_distribution_id
}
