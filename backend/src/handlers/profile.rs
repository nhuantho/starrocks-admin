use axum::{
    Json,
    extract::{Path, Query, State},
};
use std::sync::Arc;

use crate::models::{ProfileDetail, ProfileListItem};
use crate::services::profile_analyzer::doris::variables::DORIS_VARIABLE_NAMES;
use crate::services::profile_analyzer::models::ArchivedSnapshotInfo;
use crate::services::profile_analyzer::{
    AnalysisContext, ClusterVariables, ProfileAnalysisResponse, analyze_profile_with_context,
};
use crate::services::query_profile_service::{QueryProfileService, validate_explain_sql};
use crate::services::{MySQLClient, StarRocksClient};
use crate::utils::{ApiResult, Lang, error::ApiError};
use serde::{Deserialize, Serialize};

/// Profile service for the caller's active cluster, plus that cluster's identity for scoping
/// archived snapshots. Non-super-admins only ever see their organization's active cluster.
struct ProfileContext {
    service: QueryProfileService,
    cluster_id: i64,
    organization_id: Option<i64>,
}

async fn profile_context(
    state: &crate::AppState,
    org: &crate::middleware::OrgContext,
) -> ApiResult<ProfileContext> {
    let cluster = if org.is_super_admin {
        state.cluster_service.get_active_cluster().await?
    } else {
        state
            .cluster_service
            .get_active_cluster_by_org(org.organization_id)
            .await?
    };
    let pool = state.mysql_pool_manager.get_pool(&cluster).await?;
    let (cluster_id, organization_id) = (cluster.id, cluster.organization_id);
    Ok(ProfileContext {
        service: QueryProfileService {
            client: StarRocksClient::new(cluster, state.mysql_pool_manager.clone()),
            mysql: MySQLClient::from_pool(pool),
        },
        cluster_id,
        organization_id,
    })
}

async fn profile_service(
    state: &crate::AppState,
    org: &crate::middleware::OrgContext,
) -> ApiResult<QueryProfileService> {
    Ok(profile_context(state, org).await?.service)
}

#[derive(Deserialize)]
pub struct ExplainRequest {
    sql: String,
    catalog: Option<String>,
    database: Option<String>,
}

#[derive(Serialize)]
pub struct ExplainResponse {
    plan: String,
    execution_time_ms: u128,
}

pub async fn explain_query(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Json(request): Json<ExplainRequest>,
) -> ApiResult<Json<ExplainResponse>> {
    let sql = validate_explain_sql(&request.sql)?;
    let service = profile_service(&state, &org_ctx).await?;
    let mut session = service.mysql.create_session().await?;
    if let Some(catalog) = request.catalog.as_deref().filter(|value| !value.is_empty()) {
        session.use_catalog(catalog).await?;
    }
    if let Some(database) = request
        .database
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        session.use_database(database).await?;
    }
    let (_, rows, execution_time_ms) = session.execute(&format!("EXPLAIN {}", sql)).await?;
    let plan = rows
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect::<Vec<_>>()
        .join("\n");
    if plan.trim().is_empty() {
        return Err(ApiError::not_found("The server returned no EXPLAIN plan"));
    }
    Ok(Json(ExplainResponse { plan, execution_time_ms }))
}

pub async fn get_profile_status(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Path(query_id): Path<String>,
) -> ApiResult<Json<ProfileListItem>> {
    let query_id = sanitize_query_id(&query_id)?;
    let service = profile_service(&state, &org_ctx).await?;
    service.list_profiles().await?.into_iter().find(|profile| profile.query_id == query_id)
        .map(Json).ok_or_else(|| ApiError::not_found("Query is not visible in live queries or retained profile history; its final state is unknown."))
}

/// Validate and sanitize query_id to prevent SQL injection
/// StarRocks query_id format: UUID like "12345678-1234-1234-1234-123456789abc"
///
/// Returns the sanitized (trimmed) query_id as a String.
/// The sanitized version is what should be used for:
/// - SQL queries (security)
/// - API responses (consistency)
/// - Error messages (clarity)
fn sanitize_query_id(query_id: &str) -> Result<String, ApiError> {
    let id = query_id.trim();
    // Allow alphanumeric, hyphens, and underscores (UUID format)
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(ApiError::invalid_data("Invalid query_id format"));
    }
    // Return owned String to avoid lifetime issues and ensure consistency
    Ok(id.to_string())
}

// List all query profiles for a cluster
#[utoipa::path(
    get,
    path = "/api/clusters/profiles",
    responses(
        (status = 200, description = "List of query profiles", body = Vec<ProfileListItem>),
        (status = 404, description = "No active cluster found")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Profiles"
)]
pub async fn list_profiles(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
) -> ApiResult<Json<Vec<ProfileListItem>>> {
    let service = profile_service(&state, &org_ctx).await?;
    let profiles = service.list_profiles().await.map_err(|error| {
        tracing::warn!(%error, "Failed to load query profiles");
        error
    })?;
    Ok(Json(profiles))
}

// Get detailed profile for a specific query
#[utoipa::path(
    get,
    path = "/api/clusters/profiles/{query_id}",
    params(
        ("query_id" = String, Path, description = "Query ID")
    ),
    responses(
        (status = 200, description = "Query profile detail", body = ProfileDetail),
        (status = 404, description = "No active cluster found or profile not found")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Profiles"
)]
pub async fn get_profile(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Path(query_id): Path<String>,
) -> ApiResult<Json<ProfileDetail>> {
    let safe_query_id = sanitize_query_id(&query_id)?;
    let service = profile_service(&state, &org_ctx).await?;
    let profile_content = service.profile_text(&safe_query_id).await?;
    Ok(Json(ProfileDetail { query_id: safe_query_id, profile_content }))
}

/// Lightweight response for the list; the cluster still authorizes the underlying profile read.
#[utoipa::path(
    get, path = "/api/clusters/profiles/{query_id}/metadata",
    params(("query_id" = String, Path, description = "Query ID")),
    responses((status = 200, description = "Captured session metadata", body = crate::services::query_profile_service::ProfileMetadata)),
    security(("bearer_auth" = [])), tag = "Profiles"
)]
pub async fn get_profile_metadata(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Path(query_id): Path<String>,
) -> ApiResult<Json<crate::services::query_profile_service::ProfileMetadata>> {
    let query_id = sanitize_query_id(&query_id)?;
    let service = profile_service(&state, &org_ctx).await?;
    let profile = service.profile_text(&query_id).await?;
    if profile.len() > crate::services::profile_analyzer::doris::MAX_PROFILE_BYTES {
        return Err(ApiError::invalid_data("Profile exceeds the 16 MiB analysis limit"));
    }
    Ok(Json(crate::services::query_profile_service::parse_profile_metadata(&profile)))
}

/// Analyze a query profile and return structured visualization data
#[utoipa::path(
    get,
    path = "/api/clusters/profiles/{query_id}/analyze",
    params(
        ("query_id" = String, Path, description = "Query ID to analyze")
    ),
    responses(
        (status = 200, description = "Profile analysis result with execution tree"),
        (status = 404, description = "No active cluster found or profile not found"),
        (status = 500, description = "Profile parsing failed")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Profiles"
)]
pub async fn analyze_profile_handler(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Path(query_id): Path<String>,
    lang: Lang,
) -> ApiResult<Json<ProfileAnalysisResponse>> {
    let safe_query_id = sanitize_query_id(&query_id)?;
    let ctx = profile_context(&state, &org_ctx).await?;
    analyze_query(&state, &ctx, &safe_query_id, lang)
        .await
        .map(Json)
}

fn is_terminal(response: &ProfileAnalysisResponse) -> bool {
    response.summary.as_ref().is_some_and(|summary| {
        summary.is_profile_complete != Some(false)
            && matches!(
                summary.query_state.to_ascii_uppercase().as_str(),
                "FINISHED"
                    | "EOF"
                    | "OK"
                    | "SUCCESS"
                    | "FAILED"
                    | "ERROR"
                    | "ERR"
                    | "CANCELLED"
                    | "CANCELED"
                    | "ABORTED"
                    | "TIMEOUT"
            )
    })
}

fn analyze_text(
    profile: &str,
    is_doris: bool,
    context: &AnalysisContext,
) -> ApiResult<ProfileAnalysisResponse> {
    if is_doris {
        crate::services::profile_analyzer::doris::analyze(profile, context)
            .map_err(|error| ApiError::invalid_data(format!("Doris profile analysis: {}", error)))
    } else {
        analyze_profile_with_context(profile, context)
            .map_err(|e| ApiError::internal_error(format!("Analysis failed: {}", e)))
    }
}

/// Analyze the retained profile, archiving it when snapshots are enabled and the query has
/// finished. When the cluster no longer returns the profile, fall back to an archived snapshot.
async fn analyze_query(
    state: &crate::AppState,
    ctx: &ProfileContext,
    query_id: &str,
    lang: Lang,
) -> ApiResult<ProfileAnalysisResponse> {
    let store = &state.profile_snapshot_service;
    let is_doris = ctx.service.is_doris().await?;
    let profile = match ctx.service.profile_text(query_id).await {
        Ok(profile) => profile,
        // Never use the archive to bypass an access decision made by the cluster.
        Err(error @ ApiError::Unauthorized(_)) => return Err(error),
        Err(error) => {
            if let Some(snapshot) = store.get(ctx.cluster_id, query_id).await? {
                tracing::info!(query_id, "Serving archived profile snapshot");
                // Re-analyze in the caller's language; live variables may have changed since,
                // so no parameter suggestions are derived for archived profiles.
                let context = AnalysisContext { cluster_variables: None, lang };
                let mut response = analyze_text(&snapshot.profile_text, is_doris, &context)?;
                response.archived = Some(ArchivedSnapshotInfo {
                    snapshot_id: snapshot.id,
                    captured_at: snapshot.created_at,
                    reason: snapshot.reason,
                });
                return Ok(response);
            }
            return Err(error);
        },
    };
    let cluster_variables = if is_doris {
        fetch_variables(&ctx.service.mysql, DORIS_VARIABLE_NAMES).await
    } else {
        // Fetch live cluster session variables for smart parameter recommendations.
        // Graceful degradation: if fetching fails, analysis continues without variables.
        fetch_variables(&ctx.service.mysql, CLUSTER_VARIABLE_NAMES).await
    };
    let context = AnalysisContext { cluster_variables, lang };
    let response = analyze_text(&profile, is_doris, &context)?;
    if store.enabled()
        && is_terminal(&response)
        && let Err(error) = store
            .save(ctx.cluster_id, ctx.organization_id, query_id, &profile, &response, "diagnostics")
            .await
    {
        tracing::warn!(query_id, ?error, "Failed to archive profile snapshot");
    }
    Ok(response)
}

/// Parameters we query from cluster for smart recommendations
/// These are used to provide context-aware parameter suggestions
const CLUSTER_VARIABLE_NAMES: &[&str] = &[
    "query_mem_limit",
    "query_timeout",
    "enable_spill",
    "pipeline_dop",
    "parallel_fragment_exec_instance_num",
    "io_tasks_per_scan_operator",
    "enable_global_runtime_filter",
    "runtime_join_filter_push_down_limit",
    "enable_scan_datacache",
    "enable_populate_datacache",
    "enable_query_cache",
    "pipeline_profile_level",
];

/// Fetch session variables from a fixed allow-list (never user input), bounded by a timeout.
///
/// Returns `None` if the query fails or times out (graceful degradation).
/// This allows analysis to continue even if variable fetching fails,
/// though parameter recommendations may be less accurate.
async fn fetch_variables(mysql_client: &MySQLClient, names: &[&str]) -> Option<ClusterVariables> {
    // Variable names are compile-time constants, so SQL injection is not a concern here
    let sql = format!(
        "SHOW VARIABLES WHERE Variable_name IN ({})",
        names
            .iter()
            .map(|name| format!("'{}'", name))
            .collect::<Vec<_>>()
            .join(",")
    );

    match tokio::time::timeout(VARIABLE_FETCH_TIMEOUT, mysql_client.query_raw(&sql)).await {
        Ok(Ok((_, rows))) => {
            let mut variables = ClusterVariables::new();
            for row in rows {
                // SHOW VARIABLES returns: Variable_name, Value
                if row.len() >= 2 {
                    let var_name = row[0].clone();
                    let var_value = row[1].clone();
                    variables.insert(var_name, var_value);
                }
            }
            tracing::debug!(
                "Fetched {} cluster variables for smart recommendations",
                variables.len()
            );
            Some(variables)
        },
        Ok(Err(e)) => {
            tracing::warn!(
                "Failed to fetch cluster variables: {}, analysis will continue without them",
                e
            );
            None
        },
        Err(_) => {
            tracing::warn!("Fetching cluster variables timed out; analysis continues without them");
            None
        },
    }
}

const VARIABLE_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Snapshots, comparison, profile settings, recurring queries and report export
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<i64>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct SnapshotListResponse {
    /// Whether new snapshots are being archived (config `[profile_snapshots] enabled`)
    enabled: bool,
    snapshots: Vec<crate::services::profile_snapshot_service::ProfileSnapshotSummary>,
}

/// List archived profile snapshots for the active cluster
#[utoipa::path(
    get,
    path = "/api/clusters/profiles/snapshots",
    params(("limit" = Option<i64>, Query, description = "Maximum rows (1-1000, default 100)")),
    responses((status = 200, description = "Archived snapshots, newest first", body = SnapshotListResponse)),
    security(("bearer_auth" = [])),
    tag = "Profiles"
)]
pub async fn list_snapshots(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Query(query): Query<LimitQuery>,
) -> ApiResult<Json<SnapshotListResponse>> {
    let ctx = profile_context(&state, &org_ctx).await?;
    let store = &state.profile_snapshot_service;
    Ok(Json(SnapshotListResponse {
        enabled: store.enabled(),
        snapshots: store
            .list(ctx.cluster_id, query.limit.unwrap_or(100))
            .await?,
    }))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CompareRequest {
    base_query_id: String,
    target_query_id: String,
}

/// Compare two query profiles (live or archived): phase and operator deltas, new and resolved diagnostics
#[utoipa::path(
    post,
    path = "/api/clusters/profiles/compare",
    request_body = CompareRequest,
    responses((status = 200, description = "Profile comparison")),
    security(("bearer_auth" = [])),
    tag = "Profiles"
)]
pub async fn compare_profiles(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    lang: Lang,
    Json(request): Json<CompareRequest>,
) -> ApiResult<Json<crate::services::profile_analyzer::compare::ProfileComparison>> {
    let base_id = sanitize_query_id(&request.base_query_id)?;
    let target_id = sanitize_query_id(&request.target_query_id)?;
    let ctx = profile_context(&state, &org_ctx).await?;
    let base = analyze_query(&state, &ctx, &base_id, lang).await?;
    let target = analyze_query(&state, &ctx, &target_id, lang).await?;
    let mut comparison = crate::services::profile_analyzer::compare::compare(&base, &target);
    // The analyzed summary may lack an id (e.g. truncated profile); keep the requested ids.
    comparison.base_query_id = base_id;
    comparison.target_query_id = target_id;
    Ok(Json(comparison))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ProfileSettingsResponse {
    engine: String,
    /// Current values of profile-related session variables that the cluster returned
    variables: std::collections::BTreeMap<String, String>,
    /// FE configuration that controls profile retention, when readable by this account
    fe_config: std::collections::BTreeMap<String, String>,
    guidance: Vec<String>,
    #[schema(value_type = Vec<Object>)]
    snippets: Vec<crate::services::profile_analyzer::models::SqlSnippet>,
    snapshots_enabled: bool,
}

fn text<'a>(lang: Lang, en: &'a str, zh: &'a str) -> &'a str {
    if lang == Lang::Zh { zh } else { en }
}

/// Explain why a profile may be missing, using the cluster's current profile settings
#[utoipa::path(
    get,
    path = "/api/clusters/profiles/settings",
    responses((status = 200, description = "Profile collection settings and guidance", body = ProfileSettingsResponse)),
    security(("bearer_auth" = [])),
    tag = "Profiles"
)]
pub async fn profile_settings(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    lang: Lang,
) -> ApiResult<Json<ProfileSettingsResponse>> {
    let ctx = profile_context(&state, &org_ctx).await?;
    let is_doris = ctx.service.is_doris().await?;
    let snapshots_enabled = state.profile_snapshot_service.enabled();
    let mut guidance = Vec::new();
    if !is_doris {
        guidance.push(text(lang, "Profiles are collected when enable_profile=true (or for slow queries above big_query_profile_threshold) and are kept for a limited time.", "Profile 在 enable_profile=true（或慢查询超过 big_query_profile_threshold）时采集，并只保留有限时间。").to_string());
        return Ok(Json(ProfileSettingsResponse {
            engine: "starrocks".into(),
            variables: Default::default(),
            fe_config: Default::default(),
            guidance,
            snippets: Vec::new(),
            snapshots_enabled,
        }));
    }
    let variables: std::collections::BTreeMap<_, _> = fetch_variables(
        &ctx.service.mysql,
        &["enable_profile", "profile_level", "auto_profile_threshold_ms"],
    )
    .await
    .unwrap_or_default()
    .into_iter()
    .collect();
    let mut fe_config = std::collections::BTreeMap::new();
    // Read-only FE config lookup with a fixed pattern; needs ADMIN privilege and degrades silently.
    if let Ok(Ok((_, rows))) = tokio::time::timeout(
        VARIABLE_FETCH_TIMEOUT,
        ctx.service
            .mysql
            .query_raw("ADMIN SHOW FRONTEND CONFIG LIKE '%profile%'"),
    )
    .await
    {
        for row in rows {
            if let (Some(key), Some(value)) = (row.first(), row.get(1))
                && matches!(
                    key.as_str(),
                    "max_query_profile_num"
                        | "max_spilled_profile_num"
                        | "spilled_profile_storage_path"
                        | "spilled_profile_storage_limit_bytes"
                        | "profile_async_collect_expire_time_secs"
                )
            {
                fe_config.insert(key.clone(), value.clone());
            }
        }
    }
    let is_true = |v: Option<&String>| {
        v.is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "1"))
    };
    match variables.get("enable_profile") {
        Some(_) if !is_true(variables.get("enable_profile")) => guidance.push(text(lang, "enable_profile is false for this account's sessions. A profile exists only if the query's own session ran SET enable_profile = true (or it is enabled globally).", "当前账号会话的 enable_profile 为 false。只有查询所在会话执行了 SET enable_profile = true（或全局开启）才会生成 Profile。").into()),
        Some(_) => guidance.push(text(lang, "enable_profile is true for this account's sessions; other sessions (dbt, BI tools) may still have it disabled.", "当前账号会话的 enable_profile 为 true；其他会话（dbt、BI 工具）可能仍未开启。").into()),
        None => guidance.push(text(lang, "Could not read enable_profile; the query's session must have run SET enable_profile = true.", "无法读取 enable_profile；查询所在会话必须执行 SET enable_profile = true。").into()),
    }
    match variables.get("profile_level").and_then(|v| v.trim().parse::<u8>().ok()) {
        Some(level) if level < 2 => guidance.push(format!("profile_level = {level}. {}", text(lang, "On Doris 4.0+ level 2 adds per-task scanner, connector and exchange counters used by finer diagnostics.", "在 Doris 4.0+ 中，级别 2 会增加任务级 Scanner、连接器和 Exchange 计数器，用于更细的诊断。"))),
        Some(level) => guidance.push(format!("profile_level = {level}.")),
        None => {},
    }
    if let Some(threshold) = variables.get("auto_profile_threshold_ms") {
        if threshold.trim() == "-1" {
            guidance.push(text(lang, "auto_profile_threshold_ms = -1: no duration threshold filters queries when enable_profile=true.", "auto_profile_threshold_ms = -1：enable_profile=true 时不会按查询时长过滤 Profile。").into());
        } else {
            guidance.push(format!(
                "auto_profile_threshold_ms = {threshold}. {}",
                text(
                    lang,
                    "With profiling enabled, faster queries may not keep a profile.",
                    "开启 Profile 时，快于该阈值的查询可能不保留 Profile。"
                )
            ));
        }
    }
    match fe_config.get("max_query_profile_num") {
        Some(max) => guidance.push(format!("max_query_profile_num = {max}. {}", text(lang, "The FE keeps at most this many profiles; older ones are evicted.", "FE 最多保留这么多 Profile，较早的会被淘汰。"))),
        None => guidance.push(text(lang, "The FE keeps a limited number of profiles (max_query_profile_num); older ones are evicted. This account could not read the FE config.", "FE 只保留有限数量的 Profile（max_query_profile_num），较早的会被淘汰。当前账号无法读取 FE 配置。").into()),
    }
    guidance.push(text(lang, "Profiles are fetched from all FE nodes; a profile is missing if it was evicted, never collected, or not visible to the configured cluster account.", "Profile 从所有 FE 节点获取；若被淘汰、从未采集或对所配置的集群账号不可见，则无法获取。").into());
    guidance.push(text(lang, "dbt INSERT/INSERT OVERWRITE can appear as LOAD. QUERY history is aggregated across FEs; SQL profile metadata and LOAD listings come from the connected pooled FE. Check the executing FE when a LOAD is missing.", "dbt INSERT/INSERT OVERWRITE 可能显示为 LOAD。QUERY 历史跨 FE 聚合；SQL Profile 元数据和 LOAD 列表来自连接池所连接的 FE。LOAD 缺失时请检查执行该语句的 FE。").into());
    if snapshots_enabled {
        guidance.push(text(lang, "Profile snapshots are enabled: diagnostics you open for finished queries are archived here and remain available after the cluster evicts them.", "已开启 Profile 快照：已完成查询的诊断在打开时会归档，集群淘汰后仍可查看。").into());
    } else {
        guidance.push(text(lang, "Snapshots are disabled. Enable [profile_snapshots] and open diagnostics before FE eviction to archive a finished query. There is no unattended Airflow/dbt collector.", "快照未开启。开启 [profile_snapshots] 后，需在 FE 淘汰前打开已完成查询的诊断以归档。当前没有无人值守的 Airflow/dbt 采集器。").into());
    }
    let snippets = vec![
        crate::services::profile_analyzer::models::SqlSnippet {
            label: text(
                lang,
                "Enable a detailed profile for your session",
                "为当前会话开启详细 Profile",
            )
            .into(),
            sql: "SET enable_profile = true;\nSET profile_level = 2;".into(),
            scope: "session".into(),
            warning: None,
        },
        crate::services::profile_analyzer::doris::variables::global_profile_snippet(lang),
    ];
    Ok(Json(ProfileSettingsResponse {
        engine: "doris".into(),
        variables,
        fe_config,
        guidance,
        snippets,
        snapshots_enabled,
    }))
}

#[derive(Serialize, utoipa::ToSchema, Debug, PartialEq)]
pub struct RecurringQuery {
    fingerprint: String,
    /// Normalized statement with literals replaced by `?`
    sample_sql: String,
    executions: usize,
    avg_ms: f64,
    max_ms: f64,
    total_ms: f64,
    /// Most recent first, at most five
    query_ids: Vec<String>,
}

/// Group finished statements by SQL fingerprint; only fingerprints seen at least twice are kept.
fn group_recurring(items: &[(String, String, f64)], limit: usize) -> Vec<RecurringQuery> {
    use crate::services::profile_analyzer::fingerprint::{fingerprint, normalize};
    let mut groups: Vec<RecurringQuery> = Vec::new();
    for (query_id, sql, ms) in items {
        if sql.trim().is_empty() {
            continue;
        }
        let key = fingerprint(sql);
        match groups.iter_mut().find(|g| g.fingerprint == key) {
            Some(group) => {
                group.executions += 1;
                group.total_ms += ms;
                group.max_ms = group.max_ms.max(*ms);
                if group.query_ids.len() < 5 && !group.query_ids.contains(query_id) {
                    group.query_ids.push(query_id.clone());
                }
            },
            None => groups.push(RecurringQuery {
                fingerprint: key,
                sample_sql: normalize(sql).chars().take(2000).collect(),
                executions: 1,
                avg_ms: 0.0,
                max_ms: *ms,
                total_ms: *ms,
                query_ids: vec![query_id.clone()],
            }),
        }
    }
    groups.retain(|g| g.executions >= 2);
    for group in &mut groups {
        group.avg_ms = group.total_ms / group.executions as f64;
    }
    groups.sort_by(|a, b| b.total_ms.total_cmp(&a.total_ms));
    groups.truncate(limit);
    groups
}

/// Recurring statements among retained profiles and archived snapshots, ranked by total time
#[utoipa::path(
    get,
    path = "/api/clusters/profiles/recurring",
    params(("limit" = Option<i64>, Query, description = "Maximum groups (1-100, default 20)")),
    responses((status = 200, description = "Recurring statements grouped by SQL fingerprint", body = Vec<RecurringQuery>)),
    security(("bearer_auth" = [])),
    tag = "Profiles"
)]
pub async fn recurring_queries(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Query(query): Query<LimitQuery>,
) -> ApiResult<Json<Vec<RecurringQuery>>> {
    use crate::services::profile_analyzer::doris::metric::{Unit, quantity};
    let ctx = profile_context(&state, &org_ctx).await?;
    let ms = |value: &str| match quantity(value, None) {
        Some((ns, Unit::Duration)) => Some(ns / 1e6),
        // Plain numbers in history lists are milliseconds.
        Some((n, Unit::Count)) => Some(n),
        _ => None,
    };
    let mut items: Vec<(String, String, f64)> = ctx
        .service
        .list_profiles()
        .await?
        .into_iter()
        .filter(|p| p.state == "FINISHED")
        .filter_map(|p| Some((p.query_id, p.statement, ms(&p.time)?)))
        .collect();
    // Archived snapshots extend the window beyond the cluster's retention.
    for snapshot in state
        .profile_snapshot_service
        .list(ctx.cluster_id, 1000)
        .await?
    {
        if items.iter().any(|(id, _, _)| *id == snapshot.query_id) {
            continue;
        }
        if let (Some(stored), Some(total)) = (
            state
                .profile_snapshot_service
                .get(ctx.cluster_id, &snapshot.query_id)
                .await?,
            snapshot.total_ms,
        ) {
            let sql = serde_json::from_str::<ProfileAnalysisResponse>(&stored.analysis_json)
                .ok()
                .and_then(|a| a.summary)
                .map(|s| s.sql_statement)
                .unwrap_or_default();
            items.push((snapshot.query_id, sql, total));
        }
    }
    Ok(Json(group_recurring(&items, query.limit.unwrap_or(20).clamp(1, 100) as usize)))
}

#[derive(Deserialize)]
pub struct ReportQuery {
    format: Option<String>,
    redact: Option<bool>,
}

/// Export the analysis as Markdown or HTML (never includes the raw profile)
#[utoipa::path(
    get,
    path = "/api/clusters/profiles/{query_id}/report",
    params(
        ("query_id" = String, Path, description = "Query ID"),
        ("format" = Option<String>, Query, description = "md (default) or html"),
        ("redact" = Option<bool>, Query, description = "Replace SQL literals with ?")
    ),
    responses((status = 200, description = "Analysis report")),
    security(("bearer_auth" = [])),
    tag = "Profiles"
)]
pub async fn export_report(
    State(state): State<Arc<crate::AppState>>,
    axum::extract::Extension(org_ctx): axum::extract::Extension<crate::middleware::OrgContext>,
    Path(query_id): Path<String>,
    Query(query): Query<ReportQuery>,
    lang: Lang,
) -> ApiResult<impl axum::response::IntoResponse> {
    use crate::services::profile_analyzer::report;
    let safe_query_id = sanitize_query_id(&query_id)?;
    let ctx = profile_context(&state, &org_ctx).await?;
    let analysis = analyze_query(&state, &ctx, &safe_query_id, lang).await?;
    let redact = query.redact.unwrap_or(false);
    let (body, content_type, extension) = match query.format.as_deref() {
        Some("html") => (report::html(&analysis, redact, lang), "text/html; charset=utf-8", "html"),
        Some("md") | None => {
            (report::markdown(&analysis, redact, lang), "text/markdown; charset=utf-8", "md")
        },
        Some(_) => return Err(ApiError::invalid_data("format must be md or html")),
    };
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, content_type.to_string()),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"profile-{safe_query_id}.{extension}\""),
            ),
        ],
        body,
    ))
}

#[cfg(test)]
mod recurring_tests {
    use super::group_recurring;

    #[test]
    fn groups_by_fingerprint_and_drops_one_offs() {
        let items = vec![
            ("q1".to_string(), "SELECT * FROM t WHERE id = 1".to_string(), 100.0),
            ("q2".to_string(), "select * from t where id = 2".to_string(), 300.0),
            ("q3".to_string(), "SELECT 1".to_string(), 5000.0),
            ("q4".to_string(), String::new(), 1.0),
        ];
        let groups = group_recurring(&items, 10);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].executions, 2);
        assert_eq!(groups[0].avg_ms, 200.0);
        assert_eq!(groups[0].max_ms, 300.0);
        assert_eq!(groups[0].sample_sql, "select * from t where id = ?");
        assert_eq!(groups[0].query_ids, ["q1", "q2"]);
    }
}

#[cfg(test)]
mod snapshot_state_tests {
    use super::*;

    #[test]
    fn archive_requires_explicit_terminal_state_and_no_incomplete_marker() {
        let fixture = include_str!("../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt");
        let original = crate::services::profile_analyzer::doris::analyze_with(
            fixture,
            &AnalysisContext::default(),
            &Default::default(),
        )
        .unwrap();
        for state in ["RUNNING", "PENDING", "UNKNOWN", "", "unexpected"] {
            let mut response = original.clone();
            response.summary.as_mut().unwrap().query_state = state.into();
            assert!(!is_terminal(&response), "{state}");
        }
        for state in ["OK", "FINISHED", "FAILED", "CANCELLED", "TIMEOUT"] {
            let mut response = original.clone();
            response.summary.as_mut().unwrap().query_state = state.into();
            assert!(is_terminal(&response), "{state}");
            response.summary.as_mut().unwrap().is_profile_complete = Some(false);
            assert!(!is_terminal(&response), "{state}");
        }
    }
}
