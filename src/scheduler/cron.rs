//! Cron utilities for timezone-aware scheduling.

use crate::error::AgentError;
use chrono_tz::Tz;

/// Detect the local system timezone using TZ environment variable or common Linux files.
fn detect_system_timezone() -> Option<String> {
    // 1. Try TZ environment variable
    if let Ok(tz) = std::env::var("TZ")
        && !tz.is_empty()
    {
        return Some(tz);
    }
    // 2. Try reading /etc/timezone (contains name like 'Asia/Kolkata')
    if let Ok(tz) = std::fs::read_to_string("/etc/timezone") {
        let trimmed = tz.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    // 3. Try parsing symbolic link of /etc/localtime (e.g. symlink to /usr/share/zoneinfo/Asia/Kolkata)
    if let Ok(path) = std::fs::read_link("/etc/localtime")
        && let Some(path_str) = path.to_str()
        && let Some(pos) = path_str.find("zoneinfo/")
    {
        return Some(path_str[pos + 9..].to_string());
    }
    None
}

/// Helper function to parse cron expressions and calculate next run time, supporting target timezone or system timezone detection.
pub fn get_next_cron_run(
    cron_str: &str,
    timezone_name: Option<&str>,
) -> Result<chrono::DateTime<chrono::Utc>, AgentError> {
    let padded = if cron_str.split_whitespace().count() == 5 {
        format!("0 {cron_str}")
    } else {
        cron_str.to_string()
    };

    let schedule = padded.parse::<cron::Schedule>().map_err(|e| {
        AgentError::Scheduler(format!("Invalid cron expression '{}': {e}", cron_str))
    })?;

    // Determine target timezone
    let tz: Tz = match timezone_name {
        Some(name) => name
            .parse()
            .map_err(|e| AgentError::Scheduler(format!("Invalid timezone '{}': {}", name, e)))?,
        None => {
            // Attempt to find from runtime environment
            if let Some(detected) = detect_system_timezone() {
                match detected.parse::<Tz>() {
                    Ok(detected_tz) => detected_tz,
                    Err(e) => {
                        tracing::warn!(
                            "System timezone '{}' detected but could not be parsed: {}. Falling back to UTC.",
                            detected,
                            e
                        );
                        chrono_tz::UTC
                    }
                }
            } else {
                tracing::warn!(
                    "No timezone provided and system timezone could not be detected. Falling back to UTC."
                );
                chrono_tz::UTC
            }
        }
    };

    // Calculate next local run time in that timezone
    let next_in_tz = schedule.upcoming(tz).next().ok_or_else(|| {
        AgentError::Scheduler(format!(
            "Could not calculate next run time for cron '{}'",
            cron_str
        ))
    })?;

    // Convert next run time back to Utc
    Ok(next_in_tz.with_timezone(&chrono::Utc))
}
