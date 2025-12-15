use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    /// Task ID
    pub task_id: String,

    /// Current position
    pub position: Position,

    /// Checkpoint timestamp
    pub checkpoint_at: DateTime<Utc>,

    /// Statistics
    pub stats: ProgressStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Position {
    #[serde(rename = "postgresql")]
    PostgreSQL { lsn: String },

    #[serde(rename = "mysql")]
    MySQL { file: String, position: u64 },

    #[serde(rename = "mongodb")]
    MongoDB { resume_token: String },
}

impl Position {
    pub fn postgresql(lsn: impl Into<String>) -> Self {
        Position::PostgreSQL { lsn: lsn.into() }
    }

    pub fn mysql(file: impl Into<String>, position: u64) -> Self {
        Position::MySQL {
            file: file.into(),
            position,
        }
    }

    pub fn mongodb(token: impl Into<String>) -> Self {
        Position::MongoDB {
            resume_token: token.into(),
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Position::PostgreSQL { lsn } => write!(f, "pg:{lsn}"),
            Position::MySQL { file, position } => write!(f, "mysql:{file}:{position}"),
            Position::MongoDB { resume_token } => write!(f, "mongo:{resume_token}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProgressStats {
    pub events_processed: u64,
    pub events_failed: u64,
    pub bytes_processed: u64,
    pub last_event_at: Option<DateTime<Utc>>,
}

impl ProgressStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_event(&mut self, bytes: u64) {
        self.events_processed += 1;
        self.bytes_processed += bytes;
        self.last_event_at = Some(Utc::now());
    }

    pub fn record_failure(&mut self) {
        self.events_failed += 1;
    }

    pub fn success_rate(&self) -> f64 {
        let total = self.events_processed + self.events_failed;
        if total == 0 {
            0.0
        } else {
            self.events_processed as f64 / total as f64
        }
    }
}

/// Checkpoint represents a consistent state that can be restored
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub task_id: String,
    pub position: Position,
    pub created_at: DateTime<Utc>,
    pub stats: ProgressStats,
    pub metadata: serde_json::Value,
}

impl Checkpoint {
    pub fn new(task_id: String, position: Position) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_id,
            position,
            created_at: Utc::now(),
            stats: ProgressStats::new(),
            metadata: serde_json::Value::Null,
        }
    }
}
