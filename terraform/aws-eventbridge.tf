module "label_sync" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "sync"
}

# Default EventBridge envelope carries "source": "aws.events" — fulfillment's
# handler routes exactly that to FulfillRequest::Sync (main.rs), so no
# input transformer is needed or wanted.
resource "aws_cloudwatch_event_rule" "sync" {
  name                = module.label_sync.id
  description         = "Daily humble library sync + parked-claim reconcile"
  schedule_expression = var.sync_schedule_expression
  tags                = module.label_sync.tags
}

resource "aws_cloudwatch_event_target" "sync" {
  rule = aws_cloudwatch_event_rule.sync.name
  arn  = module.lambda_fulfillment.lambda_function_arn
}

resource "aws_lambda_permission" "eventbridge_sync" {
  statement_id  = "AllowEventBridgeInvoke"
  action        = "lambda:InvokeFunction"
  function_name = module.lambda_fulfillment.lambda_function_name
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.sync.arn
}
