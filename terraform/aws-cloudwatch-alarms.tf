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

# ── the lantern: the run that never happened (spec: docs/spec-lantern.md) ────────────────────
# Same instrument as the whisper's: AWS/Scheduler's OWN metrics on the lantern's OWN group, so the
# alarm cannot inherit the failure it watches for. The Wednesday heartbeat keeps the metric
# present at <=4-day gaps, so a healthy week can never show 7 empty daily buckets.
resource "aws_cloudwatch_metric_alarm" "lantern_never_ran" {
  count             = var.lantern_enabled ? 1 : 0
  alarm_name        = "${module.label_lantern.id}-never-ran"
  alarm_description = "The lantern schedule group has not fired in over a week — the run that never happened cannot announce itself."
  namespace         = "AWS/Scheduler"
  metric_name       = "InvocationAttemptCount"
  # ScheduleGroup is the ONLY dimension AWS/Scheduler emits; the lantern has its own group.
  dimensions = {
    ScheduleGroup = aws_scheduler_schedule_group.lantern[0].name
  }
  statistic           = "Sum"
  period              = 86400 # 7 daily buckets — AWS's hard cap (see whisper_never_ran)
  evaluation_periods  = 7
  threshold           = 1
  comparison_operator = "LessThanThreshold"
  treat_missing_data  = "breaching" # silence from the metric IS the alarm condition
  datapoints_to_alarm = 7           # ALL seven must breach; the Wednesday heartbeat guarantees <=4-day gaps
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  ok_actions          = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_lantern.tags
}

resource "aws_cloudwatch_metric_alarm" "lantern_target_errors" {
  count             = var.lantern_enabled ? 1 : 0
  alarm_name        = "${module.label_lantern.id}-target-errors"
  alarm_description = "A lantern schedule fired but its lambda target errored — the invoke path is broken."
  namespace         = "AWS/Scheduler"
  metric_name       = "TargetErrorCount"
  dimensions = {
    ScheduleGroup = aws_scheduler_schedule_group.lantern[0].name
  }
  statistic           = "Sum"
  period              = 86400
  evaluation_periods  = 1
  threshold           = 0
  comparison_operator = "GreaterThanThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_lantern.tags
}

# ── the switchboard 🎛️: a register that cannot announce its own darkness ──────────────────────
# `ping_msg` cannot report that `ping_msg` is dark. The only channel that does not depend on the
# thing being reported is the log itself, so this alarm rides a metric filter over the log and NOT
# the ops webhook. A surface you must visit is not an alarm, and a message that rides a register
# cannot report that register.
#
# ⚠️ FIRST log-content-derived alarm in this repo — every other one here rides a metric AWS emits
# on its own (Lambda Errors/Invocations, AWS/Scheduler). Two consequences, both measured 2026-09-23:
#
#  ① THIS TERRAFORM MANAGES NO LOG GROUP. `grep -n aws_cloudwatch_log_group terraform/*.tf` returns
#    ZERO; the group is created implicitly by Lambda. An earlier draft referenced
#    `aws_cloudwatch_log_group.fulfillment.name` and would have failed at plan time on an undeclared
#    resource. The name is derived the way `fulfillment_silent` addresses the function.
#
#  ② THE PATTERN IS A PLAIN-TEXT LITERAL, NOT `{ $.outcome = ... }`. These logs are tracing's TEXT
#    format (`main.rs` installs `tracing_subscriber::fmt()`; the crate's `json` feature is off), and
#    a CloudWatch JSON pattern matches ZERO text events — silently, forever, with the alarm sitting
#    in INSUFFICIENT_DATA wearing green. That is the exact defect the switchboard exists to remove,
#    rebuilt inside its own remedy; it was caught by measuring the subscriber rather than assuming
#    it. The token below is `fulfillment::REGISTER_UNRESOLVED_NEEDLE`, and the test
#    `the_alarm_and_the_code_agree_on_the_string` asserts the code and this file still share it.
resource "aws_cloudwatch_log_metric_filter" "register_dark" {
  name           = "${module.label_alarms.id}-register-dark"
  log_group_name = "/aws/lambda/${module.lambda_fulfillment.lambda_function_name}"

  # Only the UNRESOLVED face carries this token. `disabled` is operator-initiated silence and must
  # never page — enforced by the needle being absent from that arm, not by a clause here.
  pattern = "\"register_dark_unresolved\""

  metric_transformation {
    name      = "RegisterUnresolved" # keep in sync with the alarm's metric_name below
    namespace = "bendobundles/switchboard"
    value     = "1"
    unit      = "Count"
    # No default_value on purpose: absent data must read as "no signal", never as a stream of zeros
    # that would hold the alarm permanently OK even if the log group stopped receiving entirely.
  }
}

resource "aws_cloudwatch_metric_alarm" "register_unresolved" {
  alarm_name        = "${module.label_alarms.id}-register-unresolved"
  alarm_description = "A bendobundles notification register is configured but UNREADABLE. This alarm rides the log, not the ops webhook, because a dark ops register cannot report itself."
  namespace         = "bendobundles/switchboard"
  metric_name       = "RegisterUnresolved"
  statistic         = "Sum"
  # CloudWatch enforces TWO limits invisible to `terraform validate`: period <= 86400 AND
  # period * evaluation_periods <= 86400, both rejected only at APPLY time — documented on
  # `fulfillment_silent` above after a gate review found them the hard way. 300x1 is inside both.
  period              = 300
  evaluation_periods  = 1
  threshold           = 1
  comparison_operator = "GreaterThanOrEqualToThreshold"
  treat_missing_data  = "notBreaching"
  alarm_actions       = [aws_sns_topic.ops_alarms.arn]
  tags                = module.label_alarms.tags
}
