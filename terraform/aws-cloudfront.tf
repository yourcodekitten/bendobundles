# Pin the PR branch ref until OMBB releases the enhancement (Task 1); swap to
# `source = "bendoerr-terraform-modules/cloudfront-and-s3-origin/aws", version = "0.5.0"`
# when tagged. Tracked in the PR body checklist.
module "site" {
  source  = "git::https://github.com/bendoerr-terraform-modules/terraform-aws-cloudfront-and-s3-origin.git?ref=kitten/additional-origins"
  context = module.context.shared
  name    = "site"

  domain_zone_name = var.domain_zone_name
  domain_zone_id   = var.domain_zone_id
  use_apex_domain  = true

  # SPA deep links: /l/<token> and /admin/* are client routes; S3 objects don't
  # exist there. 403/404 -> 200 /index.html (module knob; plan-3 carry).
  enable_spa_error_handling = true
  security_headers          = "managed"

  additional_origins = [{
    origin_id   = "api"
    domain_name = local.api_origin_domain
    origin_path = "/${module.apigateway.stage_name}"
  }]

  ordered_cache_behaviors = [
    { path_pattern = "/api/*", target_origin_id = "api" },
    { path_pattern = "/admin/api/*", target_origin_id = "api" },
  ]

  providers = {
    aws.route53 = aws.route53
  }
}
