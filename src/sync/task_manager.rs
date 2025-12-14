use crate::config::Config;
use crate::error::{MeiliBridgeError, Result};
use crate::models::Position;
use crate::pipeline::PipelineOrchestrator;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};

/// Status of a sync task
#[derive(Debug, Clone)]
pub struct TaskStatus {
    pub task_id: String,
    pub table: String,
    pub state: TaskState,
    pub last_position: Option<Position>,
    pub processed_count: u64,
    pub error_count: u64,
    pub last_error: Option<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TaskState {
    Idle,
    Running,
    Failed,
    Paused,
    Completed,
}

/// Commands for task management
#[derive(Debug)]
pub enum TaskCommand {
    Start(String),                                       // Task ID
    Stop(String),                                        // Task ID
    Pause(String),                                       // Task ID
    Resume(String),                                      // Task ID
    GetStatus(String, mpsc::Sender<Option<TaskStatus>>), // Task ID
    GetAllStatuses(mpsc::Sender<Vec<TaskStatus>>),
    CreateTask(Box<crate::config::SyncTaskConfig>, mpsc::Sender<Result<()>>),
    DeleteTask(String, mpsc::Sender<Result<()>>), // Task ID
    Shutdown,
}

/// Manages sync tasks and their lifecycle
pub struct SyncTaskManager {
    config: Arc<RwLock<Config>>,
    task_statuses: Arc<RwLock<HashMap<String, TaskStatus>>>,
    pub command_tx: Option<mpsc::Sender<TaskCommand>>,
    orchestrator: Option<Arc<RwLock<PipelineOrchestrator>>>,
    shutdown_tx: Option<watch::Sender<bool>>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl SyncTaskManager {
    pub fn new(config: Config) -> Self {
        let config = Arc::new(RwLock::new(config));
        Self {
            config,
            task_statuses: Arc::new(RwLock::new(HashMap::new())),
            command_tx: None,
            orchestrator: None,
            shutdown_tx: None,
            task_handles: Vec::new(),
        }
    }

    /// Start the task manager
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting sync task manager");

        // Create command channel
        let (tx, rx) = mpsc::channel(100);
        self.command_tx = Some(tx.clone());

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Initialize task statuses
        {
            let mut statuses = self.task_statuses.write().await;
            let config = self.config.read().await;
            for task in &config.sync_tasks {
                let status = TaskStatus {
                    task_id: task.table.clone(),
                    table: task.table.clone(),
                    state: TaskState::Idle,
                    last_position: None,
                    processed_count: 0,
                    error_count: 0,
                    last_error: None,
                    started_at: chrono::Utc::now(),
                    last_updated: chrono::Utc::now(),
                };
                statuses.insert(task.table.clone(), status);
            }
        }

        // Start command processor
        let statuses = self.task_statuses.clone();
        let config = self.config.clone();
        let mut shutdown_rx_clone = shutdown_rx.clone();

        let command_handle = tokio::spawn(async move {
            Self::process_commands(rx, statuses, config, &mut shutdown_rx_clone).await;
        });

        self.task_handles.push(command_handle);

        // Start pipeline orchestrator
        let config = self.config.read().await;
        let mut orchestrator = PipelineOrchestrator::new((*config).clone())?;
        drop(config); // Release the read lock before starting
        orchestrator.start().await?;
        self.orchestrator = Some(Arc::new(RwLock::new(orchestrator)));

        // Start monitoring task
        let statuses = self.task_statuses.clone();
        let mut shutdown_rx_clone = shutdown_rx.clone();

        let monitor_handle = tokio::spawn(async move {
            Self::monitor_tasks(statuses, &mut shutdown_rx_clone).await;
        });

        self.task_handles.push(monitor_handle);

        // Auto-start tasks configured for auto-start
        {
            let config = self.config.read().await;
            for task in &config.sync_tasks {
                if task.auto_start.unwrap_or(true) {
                    info!("Auto-starting sync task for table '{}'", task.table);
                    if let Some(tx) = &self.command_tx {
                        let _ = tx.send(TaskCommand::Start(task.table.clone())).await;
                    }
                }
            }
        }

        info!("Sync task manager started");
        Ok(())
    }

    /// Stop the task manager
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping sync task manager");

        // Send shutdown signal
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(true);
        }

        // Send shutdown command
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(TaskCommand::Shutdown).await;
        }

        // Stop orchestrator
        if let Some(orchestrator_arc) = self.orchestrator.take() {
            let mut orchestrator = orchestrator_arc.write().await;
            orchestrator.stop().await?;
        }

        // Wait for tasks to complete
        for handle in self.task_handles.drain(..) {
            let _ = handle.await;
        }

        info!("Sync task manager stopped");
        Ok(())
    }

    /// Process commands
    async fn process_commands(
        mut rx: mpsc::Receiver<TaskCommand>,
        statuses: Arc<RwLock<HashMap<String, TaskStatus>>>,
        config: Arc<RwLock<Config>>,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                Some(cmd) = rx.recv() => {
                    match cmd {
                        TaskCommand::Start(task_id) => {
                            Self::start_task(&task_id, &statuses, &config).await;
                        }
                        TaskCommand::Stop(task_id) => {
                            Self::stop_task(&task_id, &statuses).await;
                        }
                        TaskCommand::Pause(task_id) => {
                            Self::pause_task(&task_id, &statuses).await;
                        }
                        TaskCommand::Resume(task_id) => {
                            Self::resume_task(&task_id, &statuses).await;
                        }
                        TaskCommand::GetStatus(task_id, resp_tx) => {
                            let statuses_map = statuses.read().await;
                            let status = statuses_map.get(&task_id).cloned();
                            let _ = resp_tx.send(status).await;
                        }
                        TaskCommand::GetAllStatuses(resp_tx) => {
                            let statuses_map = statuses.read().await;
                            let all_statuses: Vec<TaskStatus> = statuses_map.values().cloned().collect();
                            let _ = resp_tx.send(all_statuses).await;
                        }
                        TaskCommand::CreateTask(task_config, resp_tx) => {
                            let result = Self::create_task(*task_config, &statuses, &config).await;
                            let _ = resp_tx.send(result).await;
                        }
                        TaskCommand::DeleteTask(task_id, resp_tx) => {
                            let result = Self::delete_task(&task_id, &statuses, &config).await;
                            let _ = resp_tx.send(result).await;
                        }
                        TaskCommand::Shutdown => {
                            info!("Received shutdown command");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Received shutdown signal in command processor");
                        break;
                    }
                }
            }
        }
    }

    /// Start a specific task
    async fn start_task(
        task_id: &str,
        statuses: &Arc<RwLock<HashMap<String, TaskStatus>>>,
        _config: &Arc<RwLock<Config>>,
    ) {
        info!("Starting task '{}'", task_id);

        let mut statuses_map = statuses.write().await;
        if let Some(status) = statuses_map.get_mut(task_id) {
            if status.state == TaskState::Running {
                warn!("Task '{}' is already running", task_id);
                return;
            }

            status.state = TaskState::Running;
            status.last_updated = chrono::Utc::now();
            info!("Task '{}' state changed to Running", task_id);
        }
    }

    /// Stop a specific task
    async fn stop_task(task_id: &str, statuses: &Arc<RwLock<HashMap<String, TaskStatus>>>) {
        info!("Stopping task '{}'", task_id);

        let mut statuses_map = statuses.write().await;
        if let Some(status) = statuses_map.get_mut(task_id) {
            status.state = TaskState::Idle;
            status.last_updated = chrono::Utc::now();
            info!("Task '{}' state changed to Idle", task_id);
        }
    }

    /// Pause a specific task
    async fn pause_task(task_id: &str, statuses: &Arc<RwLock<HashMap<String, TaskStatus>>>) {
        info!("Pausing task '{}'", task_id);

        let mut statuses_map = statuses.write().await;
        if let Some(status) = statuses_map.get_mut(task_id)
            && status.state == TaskState::Running
        {
            status.state = TaskState::Paused;
            status.last_updated = chrono::Utc::now();
            info!("Task '{}' state changed to Paused", task_id);
        }
    }

    /// Resume a specific task
    async fn resume_task(task_id: &str, statuses: &Arc<RwLock<HashMap<String, TaskStatus>>>) {
        info!("Resuming task '{}'", task_id);

        let mut statuses_map = statuses.write().await;
        if let Some(status) = statuses_map.get_mut(task_id)
            && status.state == TaskState::Paused
        {
            status.state = TaskState::Running;
            status.last_updated = chrono::Utc::now();
            info!("Task '{}' state changed to Running", task_id);
        }
    }

    /// Monitor tasks and update statuses
    async fn monitor_tasks(
        statuses: Arc<RwLock<HashMap<String, TaskStatus>>>,
        shutdown_rx: &mut watch::Receiver<bool>,
    ) {
        let mut monitor_interval = interval(Duration::from_secs(10));

        loop {
            tokio::select! {
                _ = monitor_interval.tick() => {
                    let statuses_map = statuses.read().await;

                    // Log status summary
                    let running_count = statuses_map.values()
                        .filter(|s| s.state == TaskState::Running)
                        .count();
                    let failed_count = statuses_map.values()
                        .filter(|s| s.state == TaskState::Failed)
                        .count();

                    debug!(
                        "Task status summary: {} total, {} running, {} failed",
                        statuses_map.len(),
                        running_count,
                        failed_count
                    );

                    // Check for tasks that need recovery
                    for (task_id, status) in statuses_map.iter() {
                        if status.state == TaskState::Failed {
                            let time_since_failure = chrono::Utc::now() - status.last_updated;
                            if time_since_failure > chrono::Duration::minutes(5) {
                                warn!(
                                    "Task '{}' has been in failed state for {} minutes",
                                    task_id,
                                    time_since_failure.num_minutes()
                                );
                            }
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Received shutdown signal in monitor task");
                        break;
                    }
                }
            }
        }
    }

    /// Get status of a specific task
    pub async fn get_task_status(&self, task_id: &str) -> Option<TaskStatus> {
        if let Some(tx) = &self.command_tx {
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            let _ = tx
                .send(TaskCommand::GetStatus(task_id.to_string(), resp_tx))
                .await;
            resp_rx.recv().await.flatten()
        } else {
            None
        }
    }

    /// Get status of all tasks
    pub async fn get_all_task_statuses(&self) -> Vec<TaskStatus> {
        if let Some(tx) = &self.command_tx {
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            let _ = tx.send(TaskCommand::GetAllStatuses(resp_tx)).await;
            resp_rx.recv().await.unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    /// Update task progress
    pub async fn update_task_progress(
        &self,
        task_id: &str,
        position: Position,
        processed_count: u64,
    ) -> Result<()> {
        let mut statuses_map = self.task_statuses.write().await;
        if let Some(status) = statuses_map.get_mut(task_id) {
            status.last_position = Some(position);
            status.processed_count = processed_count;
            status.last_updated = chrono::Utc::now();
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(format!(
                "Task '{}' not found",
                task_id
            )))
        }
    }

    /// Report task error
    pub async fn report_task_error(&self, task_id: &str, error: String) -> Result<()> {
        let mut statuses_map = self.task_statuses.write().await;
        if let Some(status) = statuses_map.get_mut(task_id) {
            status.error_count += 1;
            status.last_error = Some(error);
            status.last_updated = chrono::Utc::now();

            // Change state to Failed if too many errors
            if status.error_count > 10 {
                status.state = TaskState::Failed;
                error!(
                    "Task '{}' failed after {} errors",
                    task_id, status.error_count
                );
            }

            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(format!(
                "Task '{}' not found",
                task_id
            )))
        }
    }

    /// Create a new sync task
    async fn create_task(
        task_config: crate::config::SyncTaskConfig,
        statuses: &Arc<RwLock<HashMap<String, TaskStatus>>>,
        config: &Arc<RwLock<Config>>,
    ) -> Result<()> {
        info!("Creating new sync task for table '{}'", task_config.table);

        // Add task to config
        {
            let mut config_guard = config.write().await;

            // Check if task already exists
            if config_guard
                .sync_tasks
                .iter()
                .any(|t| t.table == task_config.table)
            {
                return Err(MeiliBridgeError::Validation(format!(
                    "Task for table '{}' already exists",
                    task_config.table
                )));
            }

            config_guard.sync_tasks.push(task_config.clone());
        }

        // Add task status
        {
            let mut statuses_map = statuses.write().await;
            let status = TaskStatus {
                task_id: task_config.table.clone(),
                table: task_config.table.clone(),
                state: TaskState::Idle,
                last_position: None,
                processed_count: 0,
                error_count: 0,
                last_error: None,
                started_at: chrono::Utc::now(),
                last_updated: chrono::Utc::now(),
            };
            statuses_map.insert(task_config.table.clone(), status);
        }

        info!(
            "Successfully created sync task for table '{}'",
            task_config.table
        );
        Ok(())
    }

    /// Delete a sync task
    async fn delete_task(
        task_id: &str,
        statuses: &Arc<RwLock<HashMap<String, TaskStatus>>>,
        config: &Arc<RwLock<Config>>,
    ) -> Result<()> {
        info!("Deleting sync task '{}'", task_id);

        // First, stop the task if it's running
        Self::stop_task(task_id, statuses).await;

        // Remove from config
        {
            let mut config_guard = config.write().await;
            config_guard.sync_tasks.retain(|t| t.table != task_id);
        }

        // Remove from statuses
        {
            let mut statuses_map = statuses.write().await;
            statuses_map.remove(task_id);
        }

        info!("Successfully deleted sync task '{}'", task_id);
        Ok(())
    }

    /// Create a new sync task (public interface)
    pub async fn create_sync_task(&self, task_config: crate::config::SyncTaskConfig) -> Result<()> {
        if let Some(tx) = &self.command_tx {
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            tx.send(TaskCommand::CreateTask(Box::new(task_config), resp_tx))
                .await
                .map_err(|_| {
                    MeiliBridgeError::Pipeline("Failed to send create task command".to_string())
                })?;

            resp_rx.recv().await.ok_or_else(|| {
                MeiliBridgeError::Pipeline("No response from task manager".to_string())
            })?
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Task manager not started".to_string(),
            ))
        }
    }

    /// Delete a sync task (public interface)
    pub async fn delete_sync_task(&self, task_id: &str) -> Result<()> {
        if let Some(tx) = &self.command_tx {
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            tx.send(TaskCommand::DeleteTask(task_id.to_string(), resp_tx))
                .await
                .map_err(|_| {
                    MeiliBridgeError::Pipeline("Failed to send delete task command".to_string())
                })?;

            resp_rx.recv().await.ok_or_else(|| {
                MeiliBridgeError::Pipeline("No response from task manager".to_string())
            })?
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Task manager not started".to_string(),
            ))
        }
    }
}
