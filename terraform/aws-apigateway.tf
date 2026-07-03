module "apigateway" {
  source  = "bendoerr-terraform-modules/apigateway/aws"
  version = "1.1.1"

  # The stage's method_settings (INFO logging) 400 unless the account-level
  # CloudWatch role below exists first.
  depends_on = [aws_api_gateway_account.this]
  context    = module.context.shared
  name       = "api"

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
  statement_id  = "AllowAPIGatewayInvokePublic"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_public_api.lambda_function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${module.apigateway.rest_api_execution_arn}/*/*/*"
}

resource "aws_lambda_permission" "apigw_admin" {
  statement_id  = "AllowAPIGatewayInvokeAdmin"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_admin_api.lambda_function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${module.apigateway.rest_api_execution_arn}/*/*/*"
}

locals {
  api_origin_domain = "${module.apigateway.rest_api_id}.execute-api.${var.region}.amazonaws.com"
}

# ── Account-level API Gateway logging role ────────────────────────────────────
# Execution logging (method_settings logging_level INFO) requires a PER-REGION
# account setting pointing at a role API Gateway can assume to push logs — a
# classic one-time gotcha: without it, UpdateStage 400s with "CloudWatch Logs
# role ARN must be set in account settings". This stack owns that setting.
module "label_apigw_logs" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "apigw-logs"
}

data "aws_iam_policy_document" "apigw_logs_assume" {
  statement {
    effect  = "Allow"
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["apigateway.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "apigw_cloudwatch" {
  name               = module.label_apigw_logs.id
  assume_role_policy = data.aws_iam_policy_document.apigw_logs_assume.json
  tags               = module.label_apigw_logs.tags
}

resource "aws_iam_role_policy_attachment" "apigw_cloudwatch" {
  role       = aws_iam_role.apigw_cloudwatch.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs"
}

resource "aws_api_gateway_account" "this" {
  cloudwatch_role_arn = aws_iam_role.apigw_cloudwatch.arn
}
