# Pinned to the immutable merge sha of cf-s3-origin#137 (MERGED; the feature
# branch is deleted). Swap to
# `source = "bendoerr-terraform-modules/cloudfront-and-s3-origin/aws", version = "0.5.0"`
# the moment the release is tagged (OMBB shepherds after ben's greenlight).
module "site" {
  source  = "git::https://github.com/bendoerr-terraform-modules/terraform-aws-cloudfront-and-s3-origin.git?ref=041718bffaa9abe60693b42f3cc4644634ac0467"
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
