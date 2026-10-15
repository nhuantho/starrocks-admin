-- ========================================
-- Profile snapshots (opt-in archive of query profiles and analyses)
-- ========================================
-- Doris and StarRocks only keep a limited number of profiles. When enabled in config
-- ([profile_snapshots] enabled = true), the analyzed profile is archived here so it can be
-- reopened and compared later. Size and age caps are enforced by a periodic purge job.

CREATE TABLE IF NOT EXISTS profile_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    cluster_id INTEGER NOT NULL,
    organization_id INTEGER NULL,
    query_id VARCHAR(64) NOT NULL,
    engine VARCHAR(16) NOT NULL,
    sql_fingerprint VARCHAR(32) NOT NULL DEFAULT '',
    query_state VARCHAR(32) NOT NULL DEFAULT '',
    total_ms REAL NULL,
    profile_text TEXT NOT NULL,
    analysis_json TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    reason VARCHAR(32) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (cluster_id) REFERENCES clusters(id) ON DELETE CASCADE,
    UNIQUE (cluster_id, query_id)
);

CREATE INDEX IF NOT EXISTS idx_profile_snapshots_cluster_created ON profile_snapshots(cluster_id, created_at);
CREATE INDEX IF NOT EXISTS idx_profile_snapshots_fingerprint ON profile_snapshots(cluster_id, sql_fingerprint);
