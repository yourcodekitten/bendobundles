module "apigateway" {
  source  = "bendoerr-terraform-modules/apigateway/aws"
  version = "1.1.1"
  context = module.context.shared
  name    = "api"

  description = "bendobundles API: /api/* -> public-api, /admin/api/* -> admin-api"

  endpoint_configuration = {
    types = ["REGIONAL"] # CloudFront sits in front; EDGE would double-CDN
  }

  stage_config = {
    name = "live"
  }

  # Personal-scale throttling — spec §7 wants rate-limited token lookups.
  method_settings = {
    "*/*" = {
      throttling_rate_limit  = 25
      throttling_burst_limit = 50
    }
  }

  openapi_config = jsonencode({
    openapi = "3.0.1"
    info = {
      title   = "bendobundles"
      version = "1.0"
    }
    paths = {
      "/api/{proxy+}" = {
        x-amazon-apigateway-any-method = {
          parameters = [{
            name     = "proxy"
            in       = "path"
            required = true
            schema   = { type = "string" }
          }]
          x-amazon-apigateway-integration = {
            uri                 = "arn:aws:apigateway:${var.region}:lambda:path/2015-03-31/functions/${module.lambda_public_api.lambda_function_arn}/invocations"
            type                = "aws_proxy"
            httpMethod          = "POST"
            passthroughBehavior = "when_no_match"
            timeoutInMillis     = 29000
          }
          responses = { "200" = { description = "proxied" } }
        }
      }
      "/admin/api/{proxy+}" = {
        x-amazon-apigateway-any-method = {
          parameters = [{
            name     = "proxy"
            in       = "path"
            required = true
            schema   = { type = "string" }
          }]
          x-amazon-apigateway-integration = {
            uri                 = "arn:aws:apigateway:${var.region}:lambda:path/2015-03-31/functions/${module.lambda_admin_api.lambda_function_arn}/invocations"
            type                = "aws_proxy"
            httpMethod          = "POST"
            passthroughBehavior = "when_no_match"
            timeoutInMillis     = 29000
          }
          responses = { "200" = { description = "proxied" } }
        }
      }
    }
  })
}

# Module does NOT create integration permissions (confirmed) — caller wires them.
resource "aws_lambda_permission" "apigw_public" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_public_api.lambda_function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${module.apigateway.rest_api_execution_arn}/*/*/*"
}

resource "aws_lambda_permission" "apigw_admin" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_admin_api.lambda_function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${module.apigateway.rest_api_execution_arn}/*/*/*"
}

locals {
  api_origin_domain = "${module.apigateway.rest_api_id}.execute-api.${var.region}.amazonaws.com"
}
