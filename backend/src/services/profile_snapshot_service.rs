//! Opt-in archive of analyzed query profiles, scoped by cluster (and therefore organization).
use crate::config::ProfileSnapshotConfig;
use crate::services::profile_analyzer::ProfileAnalysisResponse;
use crate::services::profile_analyzer::fingerprint::fingerprint;
use crate::utils::scheduled_executor::ScheduledTask;
use crate::utils::{ApiError, ApiResult};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};
use std::future::Future;
use std::pin::Pin;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, FromRow, ToSchema)]
pub struct ProfileSnapshotSummary {
    pub id: i64,
    pub query_id: String,
    pub engine: String,
    pub sql_fingerprint: String,
    pub query_state: String,
    pub total_ms: Option<f64>,
    pub size_bytes: i64,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct ProfileSnapshot {
    pub id: i64,
    pub query_id: String,
    pub profile_text: String,
    pub analysis_json: String,
    pub reason: String,
    pub created_at: String,
}

pub struct ProfileSnapshotService {
    pool: SqlitePool,
    pub config: ProfileSnapshotConfig,
}

impl ProfileSnapshotService {
    pub fn new(pool: SqlitePool, config: ProfileSnapshotConfig) -> Self {
        Self { pool, config }
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Store or replace the snapshot for `(cluster_id, query_id)`. Returns false when disabled or
    /// the profile exceeds the size cap. The SQL text is stored only inside the profile itself.
    pub async fn save(
        &self,
        cluster_id: i64,
        organization_id: Option<i64>,
        query_id: &str,
        profile_text: &str,
        analysis: &ProfileAnalysisResponse,
        reason: &str,
    ) -> ApiResult<bool> {
        if !self.enabled() || profile_text.len() as i64 > self.config.max_profile_bytes {
            return Ok(false);
        }
        let summary = analysis.summary.clone().unwrap_or_default();
        let mut stored = analysis.clone();
        // The profile is stored once, in its own column.
        stored.profile_content = None;
        stored.archived = None;
        let analysis_json = serde_json::to_string(&stored)
            .map_err(|e| ApiError::internal_error(format!("Snapshot serialization failed: {e}")))?;
        sqlx::query(
            "INSERT INTO profile_snapshots (cluster_id, organization_id, query_id, engine, sql_fingerprint, query_state, total_ms, profile_text, analysis_json, size_bytes, reason, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)
             ON CONFLICT(cluster_id, query_id) DO UPDATE SET
               engine = excluded.engine, sql_fingerprint = excluded.sql_fingerprint, query_state = excluded.query_state,
               total_ms = excluded.total_ms, profile_text = excluded.profile_text, analysis_json = excluded.analysis_json,
               size_bytes = excluded.size_bytes, reason = excluded.reason, created_at = CURRENT_TIMESTAMP",
        )
        .bind(cluster_id)
        .bind(organization_id)
        .bind(query_id)
        .bind(&analysis.engine)
        .bind(fingerprint(&summary.sql_statement))
        .bind(&summary.query_state)
        .bind(summary.total_time_ms)
        .bind(profile_text)
        .bind(analysis_json)
        .bind(profile_text.len() as i64)
        .bind(reason)
        .execute(&self.pool)
        .await?;
        tracing::debug!(cluster_id, query_id, "Archived query profile snapshot");
        Ok(true)
    }

    pub async fn get(&self, cluster_id: i64, query_id: &str) -> ApiResult<Option<ProfileSnapshot>> {
        Ok(sqlx::query_as::<_, ProfileSnapshot>(
            "SELECT id, query_id, profile_text, analysis_json, reason, CAST(created_at AS TEXT) AS created_at
             FROM profile_snapshots WHERE cluster_id = ? AND query_id = ?",
        )
        .bind(cluster_id)
        .bind(query_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn list(
        &self,
        cluster_id: i64,
        limit: i64,
    ) -> ApiResult<Vec<ProfileSnapshotSummary>> {
        Ok(sqlx::query_as::<_, ProfileSnapshotSummary>(
            "SELECT id, query_id, engine, sql_fingerprint, query_state, total_ms, size_bytes, reason, CAST(created_at AS TEXT) AS created_at
             FROM profile_snapshots WHERE cluster_id = ? ORDER BY created_at DESC, id DESC LIMIT ?",
        )
        .bind(cluster_id)
        .bind(limit.clamp(1, 1000))
        .fetch_all(&self.pool)
        .await?)
    }

    /// Delete snapshots past the age cap, then the oldest ones beyond the count and byte caps.
    pub async fn purge(&self) -> ApiResult<u64> {
        let mut deleted =
            sqlx::query("DELETE FROM profile_snapshots WHERE created_at < datetime('now', ?)")
                .bind(format!("-{} days", self.config.max_age_days.max(0)))
                .execute(&self.pool)
                .await?
                .rows_affected();
        deleted += sqlx::query(
            "DELETE FROM profile_snapshots WHERE id IN (
               SELECT id FROM (
                 SELECT id,
                   ROW_NUMBER() OVER (PARTITION BY cluster_id ORDER BY created_at DESC, id DESC) AS rank,
                   SUM(size_bytes) OVER (PARTITION BY cluster_id ORDER BY created_at DESC, id DESC) AS running_bytes
                 FROM profile_snapshots)
               WHERE rank > ? OR running_bytes > ?)",
        )
        .bind(self.config.max_per_cluster.max(0))
        .bind(self.config.max_bytes_per_cluster.max(0))
        .execute(&self.pool)
        .await?
        .rows_affected();
        if deleted > 0 {
            tracing::info!(deleted, "Purged profile snapshots");
        }
        Ok(deleted)
    }
}

impl ScheduledTask for ProfileSnapshotService {
    fn run(&self) -> Pin<Box<dyn Future<Output = Result<(), anyhow::Error>> + Send + '_>> {
        Box::pin(async move {
            self.purge().await.map_err(|e| anyhow::anyhow!("{e:?}"))?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::profile_analyzer::{AnalysisContext, doris};
    use crate::tests::common::create_test_db;

    const PROFILE: &str = include_str!("../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt");

    async fn service(config: ProfileSnapshotConfig) -> (ProfileSnapshotService, SqlitePool) {
        let pool = create_test_db().await;
        for id in [1, 2] {
            sqlx::query("INSERT INTO clusters (id, name, fe_host, fe_http_port, fe_query_port, username, password_encrypted, is_active) VALUES (?, ?, 'h', 8030, 9030, 'u', 'p', 0)")
                .bind(id)
                .bind(format!("c{id}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        (ProfileSnapshotService::new(pool.clone(), config), pool)
    }

    fn analysis() -> ProfileAnalysisResponse {
        doris::analyze_with(PROFILE, &AnalysisContext::default(), &doris::DorisOptions::default())
            .unwrap()
    }

    #[tokio::test]
    async fn disabled_store_saves_nothing() {
        let (store, _) = service(ProfileSnapshotConfig::default()).await;
        assert!(
            !store
                .save(1, None, "q1", PROFILE, &analysis(), "diagnostics")
                .await
                .unwrap()
        );
        assert!(store.list(1, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn snapshots_are_scoped_by_cluster_and_upserted() {
        let config = ProfileSnapshotConfig { enabled: true, ..Default::default() };
        let (store, _) = service(config).await;
        let a = analysis();
        assert!(
            store
                .save(1, Some(7), "q1", PROFILE, &a, "diagnostics")
                .await
                .unwrap()
        );
        assert!(
            store
                .save(1, Some(7), "q1", PROFILE, &a, "terminal")
                .await
                .unwrap()
        );
        assert!(store.get(2, "q1").await.unwrap().is_none());
        let saved = store.get(1, "q1").await.unwrap().unwrap();
        assert_eq!(saved.reason, "terminal");
        assert_eq!(saved.profile_text, PROFILE);
        let stored: ProfileAnalysisResponse = serde_json::from_str(&saved.analysis_json).unwrap();
        assert!(stored.profile_content.is_none());
        let list = store.list(1, 10).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].sql_fingerprint.len(), 16);
    }

    #[tokio::test]
    async fn purge_enforces_count_bytes_and_age() {
        let config =
            ProfileSnapshotConfig { enabled: true, max_per_cluster: 2, ..Default::default() };
        let (store, pool) = service(config).await;
        let a = analysis();
        for id in ["q1", "q2", "q3"] {
            store
                .save(1, None, id, PROFILE, &a, "diagnostics")
                .await
                .unwrap();
        }
        store
            .save(2, None, "q1", PROFILE, &a, "diagnostics")
            .await
            .unwrap();
        assert_eq!(store.purge().await.unwrap(), 1);
        let left: Vec<_> = store
            .list(1, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.query_id)
            .collect();
        assert_eq!(left, ["q3", "q2"]);
        assert_eq!(store.list(2, 10).await.unwrap().len(), 1);
        sqlx::query("UPDATE profile_snapshots SET created_at = datetime('now', '-40 days') WHERE query_id = 'q2'")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(store.purge().await.unwrap(), 1);
        let bytes_capped = ProfileSnapshotService::new(
            pool,
            ProfileSnapshotConfig {
                enabled: true,
                max_bytes_per_cluster: PROFILE.len() as i64 - 1,
                ..Default::default()
            },
        );
        assert_eq!(bytes_capped.purge().await.unwrap(), 2);
    }

    #[tokio::test]
    async fn oversized_profiles_are_not_archived() {
        let config =
            ProfileSnapshotConfig { enabled: true, max_profile_bytes: 10, ..Default::default() };
        let (store, _) = service(config).await;
        assert!(
            !store
                .save(1, None, "q1", PROFILE, &analysis(), "diagnostics")
                .await
                .unwrap()
        );
    }
}
