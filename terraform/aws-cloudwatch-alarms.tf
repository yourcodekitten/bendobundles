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

# ── the attic whispers: cause ⑤, the run that never happened ────────────────────────────────
# The whisper's own no-send announcements ride the lambda; a schedule that stops firing (or a
# lapsed invoke role) produces silence with NO announcer — "a monitor whose alert path runs
# through the monitored channel reports healthy and silent identically" (family review
# 2026-08-28). These alarms trigger on the Scheduler's OWN metrics — a different instrument, so
# they cannot inherit the failure they watch for.
resource "aws_cloudwatch_metric_alarm" "whisper_never_ran" {
  count             = var.whisper_enabled ? 1 : 0
  alarm_name        = "${module.label_whisper.id}-never-ran"
  alarm_description = "The whisper schedule has not fired in over a week — the run that never happened cannot announce itself."
  namespace         = "AWS/Scheduler"
  metric_name       = "InvocationAttemptCount"
  # ScheduleGroup is the ONLY dimension AWS/Scheduler emits (measured against the docs
  # 2026-08-28 — there is NO ScheduleName dimension; a per-schedule alarm would sit on missing
  # data forever). The whisper has its own dedicated group, so this IS the whisper's metric.
  dimensions = {
    ScheduleGroup = aws_scheduler_schedule_group.whisper[0].name
  }
  statistic           = "Sum"
  period              = 86400 # 7 daily buckets — AWS's HARD CAP: the API refused 8×86400 with "Metrics cannot be checked across more than a week" (measured at first apply, 2026-08-28; the 8-day design had a day of slack the platform does not sell).
  evaluation_periods  = 7
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching" # silence from the metric IS the alarm condition
  datapoints_to_alarm = 7           # ALL seven must breach. The slack the 8th bucket used to buy now comes from the SCHEDULE: the Sunday heartbeat tick (see whisper_schedule_expression) keeps the metric present at ≤6-day gaps, so a healthy week can never show 7 empty buckets. Lower this and missing-data-as-breaching weekdays make it alarm EVERY week.
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  ok_actions          = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_whisper.tags
}

resource "aws_cloudwatch_metric_alarm" "whisper_target_errors" {
  count             = var.whisper_enabled ? 1 : 0
  alarm_name        = "${module.label_whisper.id}-target-errors"
  alarm_description = "The whisper schedule fired but its lambda target errored — the invoke path is broken."
  namespace         = "AWS/Scheduler"
  metric_name       = "TargetErrorCount"
  # ScheduleGroup is the ONLY dimension AWS/Scheduler emits (measured against the docs
  # 2026-08-28 — there is NO ScheduleName dimension; a per-schedule alarm would sit on missing
  # data forever). The whisper has its own dedicated group, so this IS the whisper's metric.
  dimensions = {
    ScheduleGroup = aws_scheduler_schedule_group.whisper[0].name
  }
  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 1
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_whisper.tags
}
