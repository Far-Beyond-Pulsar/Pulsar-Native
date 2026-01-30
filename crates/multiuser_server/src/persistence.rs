//! Database and storage persistence layer
//!
//! This module handles database operations with PostgreSQL via sqlx
//! and local file storage for snapshots (no AWS/S3).

use anyhow::{Context, Result};
// Removed AWS dependencies - using local storage only
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::config::Config;
use crate::metrics::METRICS;

/// Session record in database
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SessionRecord {
    pub id: Uuid,
    pub session_id: String,
    pub host_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub status: String,
    pub metadata: serde_json::Value,
}

/// Snapshot metadata (local file storage)
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SnapshotRecord {
    pub id: Uuid,
    pub snapshot_id: String,
    pub session_id: String,
    pub file_path: String,  // Local file path instead of S3 key
    pub size_bytes: i64,
    pub compressed: bool,
    pub hash: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

/// Persistence layer (local storage only, no AWS)
pub struct PersistenceLayer {
    db_pool: Option<Arc<PgPool>>,
    storage_dir: Option<String>,  // Local directory for snapshots
    config: Config,
}

impl PersistenceLayer {
    /// Initialize persistence layer with database and local storage
    pub async fn new(config: Config) -> Result<Self> {
        let db_pool = if let Some(ref db_url) = config.database_url {
            info!("Connecting to PostgreSQL database");
            let pool = Self::create_pool(db_url).await?;
            Self::run_migrations(&pool).await?;
            METRICS.database_health.set(1.0);
            Some(Arc::new(pool))
        } else {
            warn!("No database URL configured - persistence disabled");
            METRICS.database_health.set(0.0);
            None
        };

        // Use local storage directory instead of S3
        let storage_dir = config.storage_dir.clone();
        if storage_dir.is_some() {
            info!("Using local storage for snapshots");
        } else {
            warn!("No storage directory configured - snapshot storage disabled");
        }

        Ok(Self {
            db_pool,
            storage_dir,
            config,
        })
    }

    async fn create_pool(database_url: &str) -> Result<PgPool> {
        PgPoolOptions::new()
            .max_connections(20)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(600))
            .max_lifetime(Duration::from_secs(1800))
            .connect(database_url)
            .await
            .context("Failed to create database pool")
    }

    async fn run_migrations(pool: &PgPool) -> Result<()> {
        info!("Running database migrations");

        // Create sessions table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                session_id VARCHAR(255) UNIQUE NOT NULL,
                host_id VARCHAR(255) NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                expires_at TIMESTAMPTZ NOT NULL,
                closed_at TIMESTAMPTZ,
                status VARCHAR(50) NOT NULL DEFAULT 'active',
                metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
                CONSTRAINT valid_status CHECK (status IN ('active', 'closed', 'expired'))
            );
            "#,
        )
        .execute(pool)
        .await
        .context("Failed to create sessions table")?;

        // Create index on session_id
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_sessions_session_id ON sessions(session_id);
            "#,
        )
        .execute(pool)
        .await
        .context("Failed to create session_id index")?;

        // Create index on status for active sessions
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status) WHERE status = 'active';
            "#,
        )
        .execute(pool)
        .await
        .context("Failed to create status index")?;

        // Create snapshots table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS snapshots (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                snapshot_id VARCHAR(255) UNIQUE NOT NULL,
                session_id VARCHAR(255) NOT NULL,
                s3_key VARCHAR(1024) NOT NULL,
                s3_bucket VARCHAR(255) NOT NULL,
                size_bytes BIGINT NOT NULL,
                compressed BOOLEAN NOT NULL DEFAULT false,
                hash BYTEA NOT NULL,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
            );
            "#,
        )
        .execute(pool)
        .await
        .context("Failed to create snapshots table")?;

        // Create index on session_id for snapshots
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_snapshots_session_id ON snapshots(session_id);
            "#,
        )
        .execute(pool)
        .await
        .context("Failed to create snapshots session_id index")?;

        info!("Database migrations completed");
        Ok(())
    }

    // Removed S3 client creation - using local storage only
    // async fn create_s3_client(config: &Config) -> Result<S3Client> { ... }

    /// Create a new session record
    pub async fn create_session(
        &self,
        session_id: String,
        host_id: String,
        expires_at: DateTime<Utc>,
        metadata: serde_json::Value,
    ) -> Result<SessionRecord> {
        let pool = self
            .db_pool
            .as_ref()
            .context("Database not configured")?;

        let record = sqlx::query_as::<_, SessionRecord>(
            r#"
            INSERT INTO sessions (session_id, host_id, expires_at, metadata)
            VALUES ($1, $2, $3, $4)
            RETURNING id, session_id, host_id, created_at, expires_at, closed_at, status, metadata
            "#,
        )
        .bind(&session_id)
        .bind(&host_id)
        .bind(expires_at)
        .bind(metadata)
        .fetch_one(pool.as_ref())
        .await
        .context("Failed to create session")?;

        info!(session_id = %session_id, "Created session record");

        Ok(record)
    }

    /// Get a session by ID
    pub async fn get_session(&self, session_id: &str) -> Result<Option<SessionRecord>> {
        let pool = self
            .db_pool
            .as_ref()
            .context("Database not configured")?;

        let record = sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id, session_id, host_id, created_at, expires_at, closed_at, status, metadata
            FROM sessions
            WHERE session_id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(pool.as_ref())
        .await
        .context("Failed to get session")?;

        Ok(record)
    }

    /// Update session status
    pub async fn update_session_status(&self, session_id: &str, status: &str) -> Result<()> {
        let pool = self
            .db_pool
            .as_ref()
            .context("Database not configured")?;

        sqlx::query(
            r#"
            UPDATE sessions
            SET status = $1, closed_at = CASE WHEN $1 = 'closed' THEN NOW() ELSE closed_at END
            WHERE session_id = $2
            "#,
        )
        .bind(status)
        .bind(session_id)
        .execute(pool.as_ref())
        .await
        .context("Failed to update session status")?;

        debug!(session_id = %session_id, status = %status, "Updated session status");

        Ok(())
    }

    /// Close a session
    pub async fn close_session(&self, session_id: &str) -> Result<()> {
        self.update_session_status(session_id, "closed").await
    }

    /// List active sessions
    pub async fn list_active_sessions(&self) -> Result<Vec<SessionRecord>> {
        let pool = self
            .db_pool
            .as_ref()
            .context("Database not configured")?;

        let records = sqlx::query_as::<_, SessionRecord>(
            r#"
            SELECT id, session_id, host_id, created_at, expires_at, closed_at, status, metadata
            FROM sessions
            WHERE status = 'active' AND expires_at > NOW()
            ORDER BY created_at DESC
            "#,
        )
        .fetch_all(pool.as_ref())
        .await
        .context("Failed to list active sessions")?;

        Ok(records)
    }

    /// Expire old sessions
    pub async fn expire_old_sessions(&self) -> Result<u64> {
        let pool = self
            .db_pool
            .as_ref()
            .context("Database not configured")?;

        let result = sqlx::query(
            r#"
            UPDATE sessions
            SET status = 'expired', closed_at = NOW()
            WHERE status = 'active' AND expires_at <= NOW()
            "#,
        )
        .execute(pool.as_ref())
        .await
        .context("Failed to expire old sessions")?;

        let count = result.rows_affected();
        if count > 0 {
            info!(count = count, "Expired old sessions");
        }

        Ok(count)
    }

    /// Upload snapshot to local storage (S3 removed)
    pub async fn upload_snapshot(
        &self,
        _snapshot_id: String,
        _session_id: String,
        _data: Vec<u8>,
        _compressed: bool,
    ) -> Result<SnapshotRecord> {
        anyhow::bail!("Snapshot storage not implemented - S3 was removed, use local file storage instead")
    }

    /// Download snapshot (S3 removed - stub)
    pub async fn download_snapshot(&self, _snapshot_id: &str) -> Result<Vec<u8>> {
        anyhow::bail!("Snapshot storage not implemented - S3 was removed, use local file storage instead")
    }

    /// Delete snapshot (S3 removed - stub)
    pub async fn delete_snapshot(&self, _snapshot_id: &str) -> Result<()> {
        anyhow::bail!("Snapshot storage not implemented - S3 was removed, use local file storage instead")
    }

    /// Health check for database
    pub async fn check_database_health(&self) -> Result<bool> {
        if let Some(ref pool) = self.db_pool {
            match sqlx::query("SELECT 1").execute(pool.as_ref()).await {
                Ok(_) => {
                    METRICS.database_health.set(1.0);
                    Ok(true)
                }
                Err(e) => {
                    error!(error = %e, "Database health check failed");
                    METRICS.database_health.set(0.0);
                    Ok(false)
                }
            }
        } else {
            Ok(false)
        }
    }

    /// Health check for storage (S3 removed - always returns false)
    pub async fn check_s3_health(&self) -> Result<bool> {
        // S3 removed - no health check needed
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_persistence_layer_creation() {
        let config = Config::default();
        let result = PersistenceLayer::new(config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_session_record_serialization() {
        let record = SessionRecord {
            id: Uuid::new_v4(),
            session_id: "test-session".to_string(),
            host_id: "host-1".to_string(),
            created_at: Utc::now(),
            expires_at: Utc::now(),
            closed_at: None,
            status: "active".to_string(),
            metadata: serde_json::json!({}),
        };

        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("test-session"));
    }
}
