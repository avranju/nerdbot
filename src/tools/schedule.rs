//! Scheduling tools — schedule_job, list_jobs, delete_job, run_job_now.

use serde_json::json;

use crate::error::AgentError;
use crate::tools::traits::{Tool, ToolContext, ToolOutput};

pub struct ScheduleJob;

#[async_trait::async_trait]
impl Tool for ScheduleJob {
    fn name(&self) -> &'static str {
        "schedule_job"
    }
    fn description(&self) -> &'static str {
        "Schedule a one-shot or recurring agent task."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("schedule_job not yet implemented".into()))
    }
}

pub struct ListJobs;

#[async_trait::async_trait]
impl Tool for ListJobs {
    fn name(&self) -> &'static str {
        "list_jobs"
    }
    fn description(&self) -> &'static str {
        "List scheduled jobs."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("list_jobs not yet implemented".into()))
    }
}

pub struct DeleteJob;

#[async_trait::async_trait]
impl Tool for DeleteJob {
    fn name(&self) -> &'static str {
        "delete_job"
    }
    fn description(&self) -> &'static str {
        "Delete a scheduled job."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("delete_job not yet implemented".into()))
    }
}

pub struct RunJobNow;

#[async_trait::async_trait]
impl Tool for RunJobNow {
    fn name(&self) -> &'static str {
        "run_job_now"
    }
    fn description(&self) -> &'static str {
        "Immediately trigger a scheduled job once."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        Err(AgentError::Generic("run_job_now not yet implemented".into()))
    }
}
