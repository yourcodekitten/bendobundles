module "label_spa_rewrite" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "spa-rewrite"
}

# SPA deep links (/l/<token>, /admin, /admin/links, ...) are client routes with
# no S3 object. Rewrite extensionless viewer URIs to /index.html on the DEFAULT
# (S3) behavior only — the API behaviors never see this function, so real API
# error statuses (404 unknown-token oracle-proofing, 403s) survive intact.
# CloudFront custom_error_response could not do this: it is distribution-wide.
resource "aws_cloudfront_function" "spa_rewrite" {
  name    = module.label_spa_rewrite.id
  runtime = "cloudfront-js-2.0"
  comment = "extensionless URIs -> /index.html (SPA client routes)"
  publish = true
  code    = <<-EOT
    function handler(event) {
      var request = event.request;
      if (!request.uri.split('/').pop().includes('.')) {
        request.uri = '/index.html';
      }
      return request;
    }
  EOT
}

module "site" {
  source  = "bendoerr-terraform-modules/cloudfront-and-s3-origin/aws"
  version = "0.6.0"
  context = module.context.shared
  name    = "site"

  domain_zone_name = var.domain_zone_name
  domain_zone_id   = var.domain_zone_id
  use_apex_domain  = true

  # SPA routing via the viewer-request function below — NOT the module's
  # enable_spa_error_handling knob, which is distribution-wide and would
  # clobber API error statuses (404 token-oracle, admin 404/403).
  function_associations = [{
    event_type   = "viewer-request"
    function_arn = aws_cloudfront_function.spa_rewrite.arn
  }]
  security_headers = "managed"

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
