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

module "label_site_headers" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "site-headers"
}

# CSP derived from what the app actually loads (#81) — every source below is
# tied to a concrete code path, so a directive can be retired when its code is:
#   img-src *.steamstatic.com    — GameGrid hardcodes shared.akamai.steamstatic.com;
#                                  stored header_image/screenshots/video_thumbnail
#                                  hosts vary across the akamai/cloudflare variants
#   media-src blob: + steamstatic — hls.js plays through MSE (blob: object URL);
#                                  Safari native HLS sets video.src to the
#                                  steamstatic URL directly (MediaHeader.handlePlay)
#   connect-src steamstatic      — hls.js XHRs manifests + segments
#   worker-src blob:             — hls.js demuxer worker
#   style-src 'unsafe-inline'    — React style={} attributes (no external styles)
# No inline scripts (vite module bundle only), no data:/blob: images, no frames,
# no external fonts; both <form>s are onSubmit-handled (no action navigation).
locals {
  site_csp = join("; ", [
    "default-src 'self'",
    "script-src 'self'",
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' https://*.steamstatic.com",
    "media-src 'self' blob: https://*.steamstatic.com",
    "connect-src 'self' https://*.steamstatic.com",
    "worker-src blob:",
    "font-src 'self'",
    "object-src 'none'",
    "base-uri 'self'",
    "form-action 'self'",
    "frame-ancestors 'none'",
  ])
}

# Replaces the AWS Managed-SecurityHeadersPolicy 1:1 on the non-CSP headers
# (same HSTS max-age, nosniff, referrer, xss-protection) and adds the CSP.
# Deltas from managed: X-Frame-Options DENY instead of SAMEORIGIN (nothing
# frames this site; pairs with frame-ancestors 'none').
resource "aws_cloudfront_response_headers_policy" "site" {
  name    = module.label_site_headers.id
  comment = "managed security headers + app-derived CSP (#81)"

  security_headers_config {
    strict_transport_security {
      access_control_max_age_sec = 31536000
      override                   = true
    }
    content_type_options {
      override = true
    }
    frame_options {
      frame_option = "DENY"
      override     = true
    }
    referrer_policy {
      referrer_policy = "strict-origin-when-cross-origin"
      override        = true
    }
    xss_protection {
      protection = true
      mode_block = true
      override   = true
    }
    content_security_policy {
      content_security_policy = local.site_csp
      override                = true
    }
  }
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
  security_headers           = "custom"
  response_headers_policy_id = aws_cloudfront_response_headers_policy.site.id

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
