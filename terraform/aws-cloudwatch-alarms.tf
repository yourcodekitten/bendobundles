# The watchdog's watchdog (spec §3, OMBB's claw #3): the pending-age sweep lives
# INSIDE the fulfillment lambda — if the cron misfires, a deploy bricks the
# function, or IAM rots, the sweep dies with it and every in-process alarm dies
# too. These two alarms are the out-of-process layer: they fire when the sync
# lambda errors, or when it goes silent for 24h -- the maximum window CloudWatch
# can express (see the silent alarm's limits comment) against the daily schedule.
# Layer map: the sweep catches the claim reconcile
# never touches; these catch the reconcile that never runs.

module "label_alarms" {
  source  = "bendoerr-terraform-modules/label/null"
  version = "1.0.1"
  context = module.context.shared
  name    = "ops"
}

resource "aws_sns_topic" "ops_alarms" {
  # label name is "ops" -> id already ends "-ops"; suffix once, not twice.
  name = "${module.label_alarms.id}-alarms"
  tags = module.label_alarms.tags
}

resource "aws_sns_topic_subscription" "ops_alarms_email" {
  topic_arn = aws_sns_topic.ops_alarms.arn
  protocol  = "email"
  endpoint  = var.ops_alarm_email # ben confirms the subscription once by mail
}

resource "aws_cloudwatch_metric_alarm" "fulfillment_errors" {
  alarm_name          = "${module.label_alarms.id}-fulfillment-errors"
  alarm_description   = "bendobundles fulfillment lambda reported errors — sync/reconcile may be silently down"
  namespace           = "AWS/Lambda"
  metric_name         = "Errors"
  dimensions          = { FunctionName = module.lambda_fulfillment.lambda_function_name }
  statistic           = "Sum"
  period              = 3600
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_alarms.tags
}

resource "aws_cloudwatch_metric_alarm" "fulfillment_silent" {
  alarm_name        = "${module.label_alarms.id}-fulfillment-silent"
  alarm_description = "bendobundles fulfillment lambda has not been invoked in 24h — the daily sync (and its pending-age sweep) is not running"
  namespace         = "AWS/Lambda"
  metric_name       = "Invocations"
  dimensions        = { FunctionName = module.lambda_fulfillment.lambda_function_name }
  statistic         = "Sum"
  # 24 consecutive silent hours. CloudWatch enforces TWO limits invisible to
  # `terraform validate`: period <= 86400 AND period * evaluation_periods <= 86400
  # TOTAL -- so 3600x24 is the maximum expressible window (gate review B-2; both
  # 90000x1 and 3600x25 are rejected at the API, at apply time). Residual: with the
  # daily cron at a fixed minute, minute-level jitter can graze one false nag per
  # miss -- accepted; a false silent-alarm nag is the cheap direction.
  period              = 3600
  evaluation_periods  = 24
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching" # NO data in an hour = not invoked = counts toward the alarm
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_alarms.tags
}
