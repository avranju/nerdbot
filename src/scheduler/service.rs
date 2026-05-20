//! Scheduler service — manages job lifecycle.

use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, error, info};

use crate::error::AgentError;
use crate::scheduler::cron::get_next_cron_run;

/// Manages persistent scheduled jobs and executes them via the agent runner.
pub struct SchedulerService {
    pool: sqlx::SqlitePool,
    provider: Arc<dyn crate::llm::provider::LlmProvider>,
    registry: Arc<crate::tools::registry::ToolRegistry>,
    config: crate::config::AppConfig,
    telegram_service: crate::telegram::service::TelegramService,
    notifier: Arc<Notify>,
    shutdown_tx: broadcast::Sender<()>,
    active_task: Mutex<Option<JoinHandle<()>>>,
    telegram_token: String,
    in_flight_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl SchedulerService {
    /// Create a new SchedulerService instance.
    pub fn new(
        pool: sqlx::SqlitePool,
        provider: Arc<dyn crate::llm::provider::LlmProvider>,
        registry: Arc<crate::tools::registry::ToolRegistry>,
        config: crate::config::AppConfig,
        telegram_service: crate::telegram::service::TelegramService,
    ) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let token_env = &config.telegram.bot_token_env;
        let telegram_token = std::env::var(token_env).unwrap_or_default();
        Self {
            pool,
            provider,
            registry,
            config,
            telegram_service,
            notifier: Arc::new(Notify::new()),
            shutdown_tx,
            active_task: Mutex::new(None),
            telegram_token,
            in_flight_tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start the scheduler, performing overdue reload & recalculation and spawning the waiting loop.
    pub async fn start(&self) -> Result<(), AgentError> {
        // NOTE: The `tokio::sync::Mutex` around `active_task` guarantees complete thread-safety
        // and mutual exclusion for the `start()` operation. Any concurrent calls to `start()`
        // will safely await the lock, and subsequent callers will find `active_task` is `Some`
        // and return early, preventing duplicate background loops from being spawned.
        let mut active_task = self.active_task.lock().await;
        if active_task.is_some() {
            return Ok(());
        }

        // 1. Startup Overdue Reload & Recalculation
        info!("Performing startup overdue reload and cron recalculation");
        let jobs = crate::storage::jobs::list_all_enabled_jobs(&self.pool).await?;
        let now = chrono::Utc::now();
        
        for job in jobs {
            if let Ok(schedule_type) = job.schedule_type() {
                match schedule_type {
                    crate::scheduler::models::ScheduleType::OneShot => {
                        if job.run_at.is_some_and(|run_at| run_at < now) {
                            if self.config.scheduler.run_overdue_one_shots_on_startup {
                                info!(job_id = %job.id, "Scheduling overdue one-shot job to run immediately on startup");
                                crate::storage::jobs::update_job_next_run(&self.pool, &job.id, Some(now), true).await?;
                            } else {
                                info!(job_id = %job.id, "Marking overdue one-shot job as Missed and disabling");
                                crate::storage::jobs::update_job_run_state(
                                    &self.pool,
                                    &job.id,
                                    crate::scheduler::models::JobStatus::Missed,
                                    now,
                                    None,
                                    false
                                ).await?;
                            }
                        }
                    }
                    crate::scheduler::models::ScheduleType::Cron => {
                        if let Some(ref cron_expression) = job.cron_expression {
                            match get_next_cron_run(cron_expression, job.timezone.as_deref()) {
                                Ok(next_run) => {
                                    info!(job_id = %job.id, next_run = %next_run, "Recalculating next run time for cron job on startup");
                                    crate::storage::jobs::update_job_next_run(&self.pool, &job.id, Some(next_run), true).await?;
                                }
                                Err(e) => {
                                    error!(job_id = %job.id, error = %e, "Invalid cron on startup reload");
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2. Spawn Background Loop
        //
        // NOTE: The scheduler background loop fetches the single next upcoming enabled job at a time.
        // It spawns a background Tokio task to execute each job concurrently, ensuring the main
        // scheduler loop remains responsive. While active, the scheduler loop yields briefly and can
        // process subsequent due jobs in the next iteration. As a Phase 5 constraint, this execution is
        // highly concurrent, but future phases (Phase 6+) may introduce explicit worker pools or concurrency limits.
        let pool = self.pool.clone();
        let provider = self.provider.clone();
        let registry = self.registry.clone();
        let config = self.config.clone();
        let telegram_service = self.telegram_service.clone();
        let notifier = self.notifier.clone();
        let telegram_token = self.telegram_token.clone();
        let in_flight_tasks = self.in_flight_tasks.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            info!("Scheduler background loop started");
            loop {
                // Fetch the single next upcoming enabled job
                let next_job = match crate::storage::jobs::get_next_enabled_job(&pool).await {
                    Ok(j) => j,
                    Err(e) => {
                        error!(error = %e, "Failed to fetch next enabled job from database");
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        continue;
                    }
                };

                match next_job {
                    Some(job) => {
                        // Check for shutdown before doing any work on a due job.
                        // Without this, stop() can keep spawning new jobs during
                        // shutdown when there is a backlog of due jobs.
                        if shutdown_rx.try_recv().is_ok() {
                            info!("Scheduler loop received shutdown signal before executing due job");
                            break;
                        }

                        let now = chrono::Utc::now();
                        let next_run = job.next_run_at.unwrap_or(now);

                        if next_run <= now {
                            // Job is due for execution!
                            // Calculate next run time and enabled status upfront to avoid DB race conditions
                            let mut next_run_at = None;
                            let mut enabled = false;
                            
                            if let Ok(schedule_type) = job.schedule_type() {
                                match schedule_type {
                                    crate::scheduler::models::ScheduleType::OneShot => {
                                        // One-shots are disabled after a single run
                                    }
                                    crate::scheduler::models::ScheduleType::Cron => {
                                        if let Some(next) = job.cron_expression.as_deref().and_then(|expr| get_next_cron_run(expr, job.timezone.as_deref()).ok()) {
                                            next_run_at = Some(next);
                                            enabled = true;
                                        }
                                    }
                                }
                            }

                            // Transition DB state to Running immediately in the main thread loop
                            let start_time = chrono::Utc::now();
                            let pool_clone = pool.clone();
                            let job_id = job.id.clone();
                            if let Err(e) = crate::storage::jobs::update_job_run_state(
                                &pool,
                                &job_id,
                                crate::scheduler::models::JobStatus::Running,
                                start_time,
                                next_run_at,
                                enabled,
                            ).await {
                                error!(job_id = %job_id, error = %e, "Failed to update job status to Running");
                                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                                continue;
                            }

                            let provider_clone = provider.clone();
                            let registry_clone = registry.clone();
                            let config_clone = config.clone();
                            let telegram_service_clone = telegram_service.clone();
                            let notifier_clone = notifier.clone();
                            
                            let telegram_token_clone = telegram_token.clone();
                            let in_flight_tasks_clone = in_flight_tasks.clone();
                            let task_handle = tokio::spawn(async move {
                                info!(job_id = %job_id, "Executing scheduled job in background");
                                
                                // Run the scheduled runner
                                let loop_config = crate::agent::agent_loop::AgentLoopConfig {
                                    max_tool_iterations: config_clone.agent.max_tool_iterations,
                                };
                                
                                // Retrieve personality content
                                let personality = match tokio::fs::read_to_string(&config_clone.agent.personality_file).await {
                                    Ok(content) => content,
                                    Err(_) => "You are a helpful assistant.".to_string(),
                                };

                                let run_result = crate::scheduler::runner::run_scheduled_job(
                                     crate::scheduler::runner::RunScheduledJobInput {
                                         pool: pool_clone.clone(),
                                         provider: provider_clone,
                                         registry: registry_clone,
                                         loop_config,
                                         personality,
                                         workspace_root: config_clone.workspace.root.clone(),
                                         telegram_token: telegram_token_clone,
                                         telegram_service: telegram_service_clone,
                                         allowed_chat_ids: config_clone.telegram.allowed_chat_ids.clone(),
                                         allowed_user_ids: config_clone.telegram.allowed_user_ids.clone(),
                                     },
                                     &job_id
                                 ).await;

                                let final_status = match run_result {
                                    Ok(_) => crate::scheduler::models::JobStatus::Success,
                                    Err(_) => crate::scheduler::models::JobStatus::Failed,
                                };

                                // Save execution metrics and keep the updated next_run_at/enabled
                                if let Err(e) = crate::storage::jobs::update_job_run_state(
                                    &pool_clone,
                                    &job_id,
                                    final_status,
                                    start_time,
                                    next_run_at,
                                    enabled
                                ).await {
                                    error!(job_id = %job_id, error = %e, "Failed to update final job execution state");
                                }
                                
                                // Trigger wakeup immediately to run the next due job
                                notifier_clone.notify_one();
                            });

                            {
                                let mut tasks = in_flight_tasks_clone.lock().await;
                                tasks.retain(|h| !h.is_finished());
                                tasks.push(task_handle);
                            }

                            // Yield briefly to let the spawned execution task progress
                            tokio::task::yield_now().await;
                        } else {
                            // Job is upcoming, sleep until it becomes due
                            let sleep_duration = (next_run - now).to_std().unwrap_or(std::time::Duration::from_secs(0));
                            debug!(seconds = sleep_duration.as_secs(), "Upcoming job found, entering reactive sleep");
                            
                            tokio::select! {
                                _ = tokio::time::sleep(sleep_duration) => {
                                    // Timer expired
                                }
                                _ = notifier.notified() => {
                                    // Woken up instantly to recalculate
                                    debug!("Scheduler loop woken up via notify channel");
                                }
                                _ = shutdown_rx.recv() => {
                                    info!("Scheduler loop received shutdown signal during sleep");
                                    break;
                                }
                            }
                        }
                    }
                    None => {
                        // No active enabled jobs - wait indefinitely
                        debug!("No enabled scheduled jobs. Idle waiting.");
                        tokio::select! {
                            _ = notifier.notified() => {
                                debug!("Scheduler loop woken up from idle by new job notification");
                            }
                            _ = shutdown_rx.recv() => {
                                info!("Scheduler loop received shutdown signal during idle wait");
                                break;
                            }
                        }
                    }
                }
            }
        });

        *active_task = Some(handle);
        Ok(())
    }

    /// Stop the scheduler background loop gracefully.
    pub async fn stop(&self) -> Result<(), AgentError> {
        let mut active_task = self.active_task.lock().await;
        if let Some(handle) = active_task.take() {
            let _ = self.shutdown_tx.send(());
            let _ = handle.await;
            
            // Wait for all in-flight background job executions to finish
            let mut tasks = self.in_flight_tasks.lock().await;
            for h in tasks.drain(..) {
                let _ = h.await;
            }
            
            info!("Scheduler background loop stopped successfully");
        }
        Ok(())
    }

    /// Accessor for the immediate wakeup notifier channel.
    pub fn notifier(&self) -> Arc<Notify> {
        self.notifier.clone()
    }
}
