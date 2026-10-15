use axum::{
    Json,
    extract::{Path, State},
};
use std::sync::Arc;

use crate::models::{ProfileDetail, ProfileListItem};
use crate::services::profile_analyzer::{
    AnalysisContext, ClusterVariables, ProfileAnalysisResponse, analyze_profile_with_context,
};
use crate::services::query_profile_service::{QueryProfileService, validate_explain_sql};
use crate::services::{MySQLClient, StarRocksClient};
use crate::utils::{ApiResult, Lang, error::ApiError};
use serde::{Deserialize, Serialize};

async fn profile_service(
    state: &crate::AppState,
    org: &crate::middleware::OrgContext,
) -> ApiResult<QueryProfileService> {
    let cluster = if org.is_super_admin {
        state.cluster_service.get_active_cluster().await?
    } else {
        state
            .cluster_service
            .get_active_cluster_by_org(org.organization_id)
            .await?
    };
    let pool = state.mysql_pool_manager.get_pool(&cluster).await?;
    Ok(QueryProfileService {
        client: StarRocksClient::new(cluster, state.mysql_pool_manager.clone()),
        mysql: MySQLClient::from_pool(pool),
    })
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
    let service = profile_service(&state, &org_ctx).await?;
    let is_doris = service.is_doris().await?;
    let profile_content = service.profile_text(&safe_query_id).await?;
    if is_doris {
        let context = AnalysisContext { cluster_variables: None, lang };
        return crate::services::profile_analyzer::doris::analyze(&profile_content, &context)
            .map(Json)
            .map_err(|error| ApiError::invalid_data(format!("Doris profile analysis: {}", error)));
    }
    let mysql_client = service.mysql;

    // Fetch live cluster session variables for smart parameter recommendations
    // Graceful degradation: if fetching fails, analysis continues without variables
    let cluster_variables = fetch_cluster_variables(&mysql_client).await;

    // Build analysis context with cluster variables
    let context = AnalysisContext { cluster_variables, lang };

    // Parse the profile and return analysis with cluster context
    analyze_profile_with_context(&profile_content, &context)
        .map(Json)
        .map_err(|e| ApiError::internal_error(format!("Analysis failed: {}", e)))
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

/// Fetch relevant session variables from the cluster
///
/// Returns `None` if query fails (graceful degradation).
/// This allows analysis to continue even if variable fetching fails,
/// though parameter recommendations may be less accurate.
async fn fetch_cluster_variables(mysql_client: &MySQLClient) -> Option<ClusterVariables> {
    // Build SQL query with parameterized variable names
    // Note: Variable names are constants, so SQL injection is not a concern here
    let sql = format!(
        "SHOW VARIABLES WHERE Variable_name IN ({})",
        CLUSTER_VARIABLE_NAMES
            .iter()
            .map(|name| format!("'{}'", name))
            .collect::<Vec<_>>()
            .join(",")
    );

    match mysql_client.query_raw(&sql).await {
        Ok((_, rows)) => {
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
        Err(e) => {
            tracing::warn!(
                "Failed to fetch cluster variables: {}, analysis will continue without them",
                e
            );
            None
        },
    }
}
