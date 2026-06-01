//! Shared system-prompt assembly helpers.

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
    use super::append_timezone_context;

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
