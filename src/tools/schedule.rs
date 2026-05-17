//! Scheduling tools — schedule_job, list_jobs, delete_job, run_job_now.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

macro_rules! stub_tool {
    ($name:expr, $desc:expr, $schema:expr) => {
        fn name(&self) -> &'static str { $name }
        fn description(&self) -> &'static str { $desc }
        fn input_schema(&self) -> serde_json::Value { $schema }
        fn execute(&self, _args: serde_json::Value, _ctx: ToolContext) -> Result<ToolOutput, AgentError> {
            Err(AgentError::Generic(concat!($name, " not yet implemented").into()))
        }
    };
}

pub struct ScheduleJob;
impl Tool for ScheduleJob {
    stub_tool!("schedule_job", "Schedule a one-shot or recurring agent task.", json!({}));
}

pub struct ListJobs;
impl Tool for ListJobs {
    stub_tool!("list_jobs", "List scheduled jobs.", json!({}));
}

pub struct DeleteJob;
impl Tool for DeleteJob {
    stub_tool!("delete_job", "Delete a scheduled job.", json!({}));
}

pub struct RunJobNow;
impl Tool for RunJobNow {
    stub_tool!("run_job_now", "Immediately trigger a scheduled job once.", json!({}));
}
