use chrono::{DateTime, TimeDelta, Utc};

pub fn assert_freeze_deadline(
    until: i64,
    response_started: i64,
    invocation_started: DateTime<Utc>,
    invocation_finished: DateTime<Utc>,
    retry_after_seconds: i64,
) {
    let lower = response_started + retry_after_seconds;
    // A direct retry-duration bound ignores the time between Go's pre-attempt and freeze clock samples.
    let upper = (invocation_finished
        + (invocation_finished - invocation_started)
        + TimeDelta::seconds(retry_after_seconds))
    .timestamp();
    assert!(
        until >= lower && until <= upper,
        "freeze deadline {until} outside observed Go clock bounds [{lower}, {upper}]; invocation [{invocation_started}, {invocation_finished}]"
    );
}
