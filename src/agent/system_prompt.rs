//! Shared system-prompt assembly helpers.

use chrono_tz::Tz;
use std::str::FromStr;

/// Return the current date and time in the given timezone as a human-readable string.
///
/// Falls back to UTC if the timezone string is not a valid `chrono_tz` timezone.
pub fn current_datetime_in_timezone(timezone: &str) -> String {
    let tz: Tz = Tz::from_str(timezone).unwrap_or(chrono_tz::UTC);

    let now = chrono::Utc::now();
    let local = now.with_timezone(&tz);

    format!(
        "## Current Date/Time in {tz} timezone: {}.",
        local.format("%A, %d-%B-%Y %H:%M:%S %Z (%:z)")
    )
}

/// Append runtime context that should be visible to the LLM on every run.
pub fn append_timezone_context(personality: &str, timezone: &str) -> String {
    let runtime_context = format!(
        "## Runtime Context\n\
         - Timezone: `{timezone}`. Interpret relative dates and times in this timezone unless the user explicitly requests another timezone."
    );

    if personality.trim().is_empty() {
        runtime_context
    } else {
        format!("{}\n\n{runtime_context}", personality.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::{append_timezone_context, current_datetime_in_timezone};

    #[test]
    fn current_datetime_in_timezone_contains_expected_format() {
        let result = current_datetime_in_timezone("Asia/Kolkata");

        // Should contain the day name
        assert!(
            result.contains("Monday")
                || result.contains("Tuesday")
                || result.contains("Wednesday")
                || result.contains("Thursday")
                || result.contains("Friday")
                || result.contains("Saturday")
                || result.contains("Sunday")
        );
        // Should contain the timezone name
        assert!(result.contains("Asia/Kolkata"));
        // Should contain month name
        assert!(
            result.contains("January")
                || result.contains("February")
                || result.contains("March")
                || result.contains("April")
                || result.contains("May")
                || result.contains("June")
                || result.contains("July")
                || result.contains("August")
                || result.contains("September")
                || result.contains("October")
                || result.contains("November")
                || result.contains("December")
        );
        // Should contain the UTC offset and not the old hour12 boolean bug.
        assert!(result.contains("+05:30"));
        assert!(!result.contains("true"));
        assert!(!result.contains("false"));
    }

    #[test]
    fn current_datetime_in_timezone_falls_back_to_utc_on_invalid_tz() {
        let result = current_datetime_in_timezone("NotARealTimezone");
        assert!(result.contains("UTC"));
    }

    #[test]
    fn appends_timezone_runtime_context_to_personality() {
        let prompt = append_timezone_context("You are helpful.", "Asia/Kolkata");

        assert!(prompt.starts_with("You are helpful.\n\n## Runtime Context"));
        assert!(prompt.contains("Timezone: `Asia/Kolkata`"));
        assert!(prompt.contains("unless the user explicitly requests another timezone"));
    }

    #[test]
    fn uses_runtime_context_as_prompt_when_personality_is_empty() {
        let prompt = append_timezone_context("", "UTC");

        assert!(prompt.starts_with("## Runtime Context"));
        assert!(prompt.contains("Timezone: `UTC`"));
    }
}
