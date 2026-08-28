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

# ── the attic whispers (spec: docs/spec-attic-whispers.md) ──────────────────────────────────
module "label_whisper" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "whisper"
}

resource "aws_iam_role" "whisper_scheduler" {
  count = var.whisper_enabled ? 1 : 0
  name  = "${module.label_whisper.id}-scheduler"
  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "scheduler.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
  tags = module.label_whisper.tags
}

resource "aws_iam_role_policy" "whisper_scheduler_invoke" {
  count = var.whisper_enabled ? 1 : 0
  name  = "invoke-fulfillment"
  role  = aws_iam_role.whisper_scheduler[0].id
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = "lambda:InvokeFunction"
      Resource = module.lambda_fulfillment.lambda_function_arn
    }]
  })
}

# Scheduler, NOT a classic rule: schedule_expression_timezone makes "Saturday morning" mean
# Saturday morning through DST (classic EventBridge rules are UTC-only — corroborated twice at
# family review, 2026-08-28). The static input is caught by fulfillment's TYPED parse before the
# `aws.events`→Sync fallback, so a whisper tick can never contaminate sync.
# Its OWN schedule group, not `default` — measured (AWS docs 2026-08-28): AWS/Scheduler metrics
# carry EXACTLY ONE dimension, ScheduleGroup; there is no per-schedule dimension. The never-ran
# alarm therefore watches the GROUP, and a group shared with any future schedule would mask the
# whisper's silence (the other schedule's invocations keep the metric non-zero). A dedicated
# group makes the group-scoped metric BE the whisper's metric, by construction.
resource "aws_scheduler_schedule_group" "whisper" {
  count = var.whisper_enabled ? 1 : 0
  name  = module.label_whisper.id
  tags  = module.label_whisper.tags
}

resource "aws_scheduler_schedule" "whisper" {
  count                        = var.whisper_enabled ? 1 : 0
  name                         = module.label_whisper.id
  group_name                   = aws_scheduler_schedule_group.whisper[0].name
  schedule_expression          = var.whisper_schedule_expression
  schedule_expression_timezone = "America/New_York"

  flexible_time_window {
    mode = "OFF"
  }

  target {
    arn      = module.lambda_fulfillment.lambda_function_arn
    role_arn = aws_iam_role.whisper_scheduler[0].arn
    input    = jsonencode({ op = "whisper" })
  }
}
