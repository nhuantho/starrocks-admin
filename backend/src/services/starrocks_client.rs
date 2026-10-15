use crate::models::{Backend, Cluster, Frontend, Query, RuntimeInfo};
use crate::services::{mysql_client::MySQLClient, mysql_pool_manager::MySQLPoolManager};
use crate::utils::{ApiError, ApiResult};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

// Doris's processlist spans all FE nodes and is independent of the SQL editor's catalog.
const DORIS_RUNNING_QUERIES_SQL: &str = r#"
SELECT *
FROM internal.information_schema.processlist
WHERE UPPER(`COMMAND`) IN ('QUERY', 'EXECUTE')
ORDER BY `TIME` DESC
"#;

fn normalize_doris_queries(rows: Vec<Value>) -> ApiResult<Vec<Value>> {
    rows.into_iter()
        .map(|row| {
            // Doris releases expose both QueryId-style and QUERY_ID-style column names.
            let field = |names: &[&str]| -> Option<String> {
                row.as_object()?.iter().find_map(|(key, value)| {
                    let key = key.replace('_', "");
                    if !names.iter().any(|name| key.eq_ignore_ascii_case(name)) || value.is_null() {
                        return None;
                    }
                    let value = value
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| value.to_string());
                    (value != "NULL").then_some(value)
                })
            };
            let required = |name: &str, aliases: &[&str]| -> ApiResult<String> {
                field(aliases).ok_or_else(|| {
                    ApiError::cluster_connection_failed(format!(
                        "Doris processlist is missing {}",
                        name
                    ))
                })
            };
            let seconds = required("Time", &["Time"])?;
            let milliseconds = seconds
                .parse::<u64>()
                .ok()
                .and_then(|seconds| seconds.checked_mul(1000))
                .ok_or_else(|| {
                    ApiError::cluster_connection_failed(
                        "Invalid Doris processlist Time".to_string(),
                    )
                })?;
            Ok(serde_json::json!({
                "QueryId": required("QueryId", &["QueryId"])?,
                "ConnectionId": required("Id", &["Id", "ConnectionId"])?,
                "User": required("User", &["User"])?,
                "Catalog": field(&["Catalog", "CatalogName"]).unwrap_or_default(),
                "Database": field(&["Db", "Database"]).unwrap_or_default(),
                "ExecTime": milliseconds.to_string(),
                "Sql": field(&["Info", "Sql"]).unwrap_or_default(),
                "feIp": field(&["FE", "FrontendInstance"]),
            }))
        })
        .collect()
}

fn parse_running_queries(rows: Vec<Value>) -> ApiResult<Vec<Query>> {
    rows.into_iter()
        .map(|row| {
            serde_json::from_value::<Query>(row).map_err(|error| {
                ApiError::cluster_connection_failed(format!(
                    "Invalid running query response: {}",
                    error
                ))
            })
        })
        .filter_map(|result| match result {
            // Omit this monitoring statement itself, without excluding same-ID sessions on other FEs.
            Ok(query)
                if query.sql.trim().trim_end_matches(';') == DORIS_RUNNING_QUERIES_SQL.trim() =>
            {
                None
            },
            result => Some(result),
        })
        .collect()
}

fn global_queries_path_missing(error: &ApiError) -> bool {
    let message = error.to_string().to_lowercase();
    message.contains("/global_current_queries")
        && (message.contains("doesn't exist") || message.contains("wrong proc path"))
}

#[cfg(test)]
mod running_query_tests {
    use super::{
        DORIS_RUNNING_QUERIES_SQL, global_queries_path_missing, normalize_doris_queries,
        parse_running_queries,
    };
    use crate::utils::ApiError;
    use serde_json::json;

    #[test]
    fn retains_queries_from_all_catalogs_and_frontends() {
        let rows = vec![
            json!({"QueryId": "q-internal", "ConnectionId": "7", "Catalog": "internal", "Database": "app", "User": "analyst", "ExecTime": "12000", "Sql": "select * from internal.app.events", "feIp": "fe-1"}),
            json!({"QueryId": "q-iceberg", "ConnectionId": "7", "Catalog": "iceberg", "Database": "app", "User": "analyst", "ExecTime": "8000", "Sql": "select * from iceberg.app.events", "feIp": "fe-2"}),
            json!({"QueryId": "q-jdbc", "ConnectionId": "9", "Catalog": "jdbc", "Database": "app", "User": "etl", "ExecTime": "5000", "Sql": "select * from jdbc.app.events", "feIp": "fe-3"}),
        ];
        let queries = parse_running_queries(rows).unwrap();
        assert_eq!(queries.len(), 3);
        assert_eq!(
            queries
                .iter()
                .map(|query| query.catalog.as_str())
                .collect::<Vec<_>>(),
            ["internal", "iceberg", "jdbc"]
        );
        assert_eq!(queries[1].connection_id, queries[0].connection_id);
        assert_ne!(queries[1].fe_ip, queries[0].fe_ip);
        assert_eq!(queries[0].exec_time, "12000");
        assert_eq!(queries[0].scan_bytes, "");
    }

    #[test]
    fn hides_only_the_monitoring_statement() {
        let rows = vec![
            json!({"QueryId": "observer", "ConnectionId": "7", "User": "root", "Sql": DORIS_RUNNING_QUERIES_SQL}),
            json!({"QueryId": "work", "ConnectionId": "7", "User": "root", "Sql": "select sleep(10)", "Catalog": "internal"}),
        ];
        let queries = parse_running_queries(rows).unwrap();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].query_id, "work");
    }

    #[test]
    fn accepts_starrocks_rows_and_reports_invalid_responses() {
        let queries = parse_running_queries(vec![json!({"QueryId": "sr-query", "ConnectionId": "42", "User": "analyst", "ScanRows": "123", "ScanBytes": "5 MB", "ExecTime": "2.5 s"})]).unwrap();
        assert_eq!(queries[0].process_rows, "123");
        assert_eq!(queries[0].catalog, "");
        assert!(parse_running_queries(vec![json!({"QueryId": "missing-fields"})]).is_err());
        assert!(parse_running_queries(Vec::new()).unwrap().is_empty());
    }

    #[test]
    fn fallback_does_not_hide_permission_or_network_errors() {
        assert!(global_queries_path_missing(&ApiError::internal_error(
            "Wrong proc path: /global_current_queries"
        )));
        for message in [
            "Access denied to /global_current_queries",
            "Connection timed out",
            "Wrong proc path: /current_queries",
        ] {
            assert!(!global_queries_path_missing(&ApiError::internal_error(message)));
        }
    }

    #[test]
    fn accepts_doris_camel_case_and_underscore_columns() {
        let rows = vec![
            json!({"Id": "7", "User": "analyst", "Catalog": "internal", "Db": "app", "Time": "12", "QueryId": "q-internal", "Info": "select sleep(30)", "FE": "fe-1"}),
            json!({"ID": 7, "USER": "analyst", "CATALOG": "iceberg", "DB": "NULL", "TIME": 8, "QUERY_ID": "q-iceberg", "INFO": "select sleep(30)", "FE": "fe-2"}),
        ];
        let queries = parse_running_queries(normalize_doris_queries(rows).unwrap()).unwrap();
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0].catalog, "internal");
        assert_eq!(queries[0].exec_time, "12000");
        assert_eq!(queries[1].catalog, "iceberg");
        assert_eq!(queries[1].exec_time, "8000");
        assert_eq!(queries[1].database, "");
        assert_eq!(queries[1].fe_ip.as_deref(), Some("fe-2"));
    }

    #[test]
    fn malformed_doris_processlist_is_not_an_empty_query_list() {
        for row in [
            json!({"Time": "12", "User": "analyst", "Id": "7"}),
            json!({"Time": "invalid", "User": "analyst", "Id": "7", "QueryId": "q1"}),
            json!({"Time": "18446744073709551615", "User": "analyst", "Id": "7", "QueryId": "q1"}),
        ] {
            assert!(normalize_doris_queries(vec![row]).is_err());
        }
    }
}

// StarRocks returns the runtime object directly; Doris wraps it in a result envelope.
fn parse_runtime_info(payload: Value) -> ApiResult<RuntimeInfo> {
    let data = if payload.get("code").is_some() || payload.get("data").is_some() {
        if payload.get("code").and_then(Value::as_i64) != Some(0) {
            return Err(ApiError::cluster_connection_failed(
                "FE runtime API returned an unsuccessful or invalid status code".to_string(),
            ));
        }
        payload.get("data").cloned().unwrap_or(Value::Null)
    } else {
        payload
    };

    serde_json::from_value(data).map_err(|e| {
        ApiError::cluster_connection_failed(format!("Failed to parse runtime response: {}", e))
    })
}

fn compute_nodes_path_missing(error: &ApiError) -> bool {
    let message = error.to_string();
    message.contains("Proc path '/compute_nodes' doesn't exist")
}

#[cfg(test)]
mod runtime_info_tests {
    use super::parse_runtime_info;
    use serde_json::json;

    #[test]
    fn accepts_starrocks_runtime() {
        let info = parse_runtime_info(json!({
            "fe_node": "fe-1", "total_mem": "1024", "free_mem": "256", "thread_cnt": "98"
        }))
        .unwrap();
        assert_eq!(info.fe_node, "fe-1");
        assert_eq!((info.total_mem, info.free_mem, info.thread_cnt), (1024, 256, 98));
    }

    #[test]
    fn accepts_doris_runtime_envelope() {
        let info = parse_runtime_info(json!({
            "msg": "success", "code": 0, "count": 0,
            "data": {"total_mem": "1037959168", "free_mem": "855642056",
                     "thread_cnt": "98", "max_mem": "1037959168"}
        }))
        .unwrap();
        assert_eq!(info.fe_node, "");
        assert_eq!((info.total_mem, info.free_mem, info.thread_cnt), (1037959168, 855642056, 98));
    }

    #[test]
    fn rejects_failed_or_malformed_runtime_responses() {
        for payload in [
            json!({"code": 401, "msg": "Unauthorized", "data": {
                "total_mem": "1024", "free_mem": "256", "thread_cnt": "98"}}),
            json!({"data": {"total_mem": "1024", "free_mem": "256", "thread_cnt": "98"}}),
            json!({"code": 0, "data": null}),
            json!({"code": 0, "data": {"free_mem": "256", "thread_cnt": "98"}}),
            json!({"total_mem": "invalid", "free_mem": "256", "thread_cnt": "98"}),
            json!({}),
        ] {
            assert!(parse_runtime_info(payload.clone()).is_err(), "accepted {payload}");
        }
    }

    #[test]
    fn only_falls_back_for_missing_compute_nodes_path() {
        use crate::utils::ApiError;
        assert!(super::compute_nodes_path_missing(&ApiError::internal_error(
            "SQL execution failed: Server error: errCode = 2, detailMessage = Proc path '/compute_nodes' doesn't exist".to_string()
        )));
        for message in
            ["Access denied", "Connection timed out", "Proc path '/backends' doesn't exist"]
        {
            assert!(!super::compute_nodes_path_missing(&ApiError::internal_error(
                message.to_string()
            )));
        }
    }

    #[test]
    fn accepts_backend_host_from_both_engines() {
        for column in ["IP", "Host"] {
            let backend: crate::models::Backend = serde_json::from_value(json!({
                (column): "be-1", "BackendId": "10001", "Alive": "true"
            }))
            .unwrap();
            assert_eq!(backend.host, "be-1");
            assert_eq!(backend.backend_id, "10001");
        }
    }
}

pub struct StarRocksClient {
    pub http_client: Client,
    pub cluster: Cluster,
    mysql_pool_manager: Arc<MySQLPoolManager>,
}

impl StarRocksClient {
    pub fn new(cluster: Cluster, mysql_pool_manager: Arc<MySQLPoolManager>) -> Self {
        let http_client = Client::builder()
            .timeout(Duration::from_secs(cluster.connection_timeout as u64))
            .build()
            .unwrap_or_else(|e| {
                // HTTP client build failure is rare and usually indicates system resource issues
                tracing::error!(
                    "Failed to build HTTP client for cluster {}: {}. This is a critical error.",
                    cluster.name,
                    e
                );
                // Use default client as fallback but log the issue
                tracing::warn!("Using default HTTP client configuration as fallback");
                Client::default()
            });

        Self { http_client, cluster, mysql_pool_manager }
    }

    pub fn get_base_url(&self) -> String {
        let protocol = if self.cluster.enable_ssl { "https" } else { "http" };
        format!("{}://{}:{}", protocol, self.cluster.fe_host, self.cluster.fe_http_port)
    }

    async fn mysql_client(&self) -> ApiResult<MySQLClient> {
        let pool = self.mysql_pool_manager.get_pool(&self.cluster).await?;
        Ok(MySQLClient::from_pool(pool))
    }

    fn normalize_proc_path(path: &str) -> String {
        if path.is_empty() {
            "/".to_string()
        } else if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{}", path)
        }
    }

    fn escape_proc_path(path: &str) -> String {
        path.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn build_show_proc_sql(path: &str) -> String {
        let normalized = Self::normalize_proc_path(path);
        let escaped = Self::escape_proc_path(&normalized);
        format!("SHOW PROC \"{}\"", escaped)
    }

    pub async fn show_proc_raw(&self, path: &str) -> ApiResult<Vec<Value>> {
        let sql = Self::build_show_proc_sql(path);
        let mysql_client = self.mysql_client().await?;
        mysql_client.query(&sql).await
    }

    async fn show_proc_entities<T>(&self, path: &str) -> ApiResult<Vec<T>>
    where
        T: DeserializeOwned,
    {
        let rows = self.show_proc_raw(path).await?;
        let mut entities = Vec::with_capacity(rows.len());

        for row in rows {
            match serde_json::from_value::<T>(row) {
                Ok(value) => entities.push(value),
                Err(e) => {
                    tracing::warn!("Failed to deserialize SHOW PROC '{}' row: {}", path, e);
                },
            }
        }

        Ok(entities)
    }

    pub async fn get_backends(&self) -> ApiResult<Vec<Backend>> {
        // Automatically switch query strategy based on deployment mode
        if self.cluster.is_shared_data() {
            tracing::info!(
                "Cluster {} is in shared-data mode, fetching compute nodes",
                self.cluster.name
            );
            return self.get_compute_nodes().await;
        }

        tracing::debug!(
            "Cluster {} is in shared-nothing mode, fetching backends",
            self.cluster.name
        );
        self.show_proc_entities::<Backend>("/backends").await
    }

    // Get compute nodes for shared-data architecture
    async fn get_compute_nodes(&self) -> ApiResult<Vec<Backend>> {
        let compute_nodes = match self.show_proc_entities::<Backend>("/compute_nodes").await {
            Ok(nodes) => nodes,
            Err(error) if compute_nodes_path_missing(&error) => {
                // Doris exposes BE nodes through /backends, including in decoupled deployments.
                tracing::debug!("No /compute_nodes proc path; fetching /backends instead");
                self.show_proc_entities::<Backend>("/backends").await?
            },
            Err(error) => return Err(error),
        };
        tracing::info!("Retrieved {} compute nodes (shared-data mode)", compute_nodes.len());
        Ok(compute_nodes)
    }

    // Execute SQL command via HTTP API
    pub async fn execute_sql(&self, sql: &str) -> ApiResult<()> {
        let url = format!("{}/api/query", self.get_base_url());
        tracing::debug!("Executing SQL: {}", sql);

        let body = serde_json::json!({
            "query": sql
        });

        let response = self
            .http_client
            .post(&url)
            .basic_auth(&self.cluster.username, Some(&self.cluster.password_encrypted))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                tracing::error!("Failed to execute SQL: {}", e);
                ApiError::cluster_connection_failed(format!("Request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            tracing::error!("SQL execution failed with status {}: {}", status, error_text);
            return Err(ApiError::cluster_connection_failed(format!(
                "SQL execution failed: {}",
                error_text
            )));
        }

        tracing::info!("SQL executed successfully: {}", sql);
        Ok(())
    }

    // Drop backend node (BE for shared-nothing, CN for shared-data)
    pub async fn drop_backend(&self, host: &str, heartbeat_port: &str) -> ApiResult<()> {
        let sql = if self.cluster.is_shared_data() {
            // Shared-data mode: drop compute node
            format!("ALTER SYSTEM DROP COMPUTE NODE \"{}:{}\"", host, heartbeat_port)
        } else {
            // Shared-nothing mode: drop backend
            format!("ALTER SYSTEM DROP BACKEND \"{}:{}\"", host, heartbeat_port)
        };

        tracing::info!(
            "Dropping {} node {}:{} from cluster {} (mode: {})",
            if self.cluster.is_shared_data() { "compute" } else { "backend" },
            host,
            heartbeat_port,
            self.cluster.name,
            self.cluster.deployment_mode
        );
        self.execute_sql(&sql).await
    }

    pub async fn get_frontends(&self) -> ApiResult<Vec<Frontend>> {
        tracing::debug!("Fetching frontends via MySQL SHOW PROC");
        self.show_proc_entities::<Frontend>("/frontends").await
    }

    // Get running queries across catalogs and FE nodes, using the connection account's visibility.
    pub async fn get_queries(&self) -> ApiResult<Vec<Query>> {
        let mysql_client = self.mysql_client().await?;
        let (columns, _) = mysql_client.query_raw("SHOW CATALOGS").await?;
        let rows = if crate::services::mysql_client::is_doris_catalogs(&columns)? {
            normalize_doris_queries(mysql_client.query(DORIS_RUNNING_QUERIES_SQL).await?)?
        } else {
            match self.show_proc_raw("/global_current_queries").await {
                Ok(rows) => rows,
                Err(error) if global_queries_path_missing(&error) => {
                    self.show_proc_raw("/current_queries").await?
                },
                Err(error) => return Err(error),
            }
        };
        parse_running_queries(rows)
    }

    // Get runtime info
    pub async fn get_runtime_info(&self) -> ApiResult<RuntimeInfo> {
        let url = format!("{}/api/show_runtime_info", self.get_base_url());

        let response = self
            .http_client
            .get(&url)
            .basic_auth(&self.cluster.username, Some(&self.cluster.password_encrypted))
            .send()
            .await
            .map_err(|e| ApiError::cluster_connection_failed(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ApiError::cluster_connection_failed(format!(
                "HTTP status: {}",
                response.status()
            )));
        }

        let payload: Value = response.json().await.map_err(|e| {
            ApiError::cluster_connection_failed(format!("Failed to parse response: {}", e))
        })?;

        parse_runtime_info(payload)
    }

    // Get metrics in Prometheus format
    pub async fn get_metrics(&self) -> ApiResult<String> {
        let url = format!("{}/metrics", self.get_base_url());

        let response = self
            .http_client
            .get(&url)
            .basic_auth(&self.cluster.username, Some(&self.cluster.password_encrypted))
            .send()
            .await
            .map_err(|e| ApiError::cluster_connection_failed(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(ApiError::cluster_connection_failed(format!(
                "HTTP status: {}",
                response.status()
            )));
        }

        let metrics_text = response.text().await.map_err(|e| {
            ApiError::cluster_connection_failed(format!("Failed to read response: {}", e))
        })?;

        Ok(metrics_text)
    }

    // Parse Prometheus metrics format
    pub fn parse_prometheus_metrics(
        &self,
        metrics_text: &str,
    ) -> ApiResult<std::collections::HashMap<String, f64>> {
        let mut metrics = std::collections::HashMap::new();

        for line in metrics_text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse format: metric_name{labels} value
            if let Some((name_part, value_str)) = line.rsplit_once(' ')
                && let Ok(value) = value_str.parse::<f64>()
            {
                // Extract metric name (before '{' or the whole name_part)
                let metric_name =
                    if let Some(pos) = name_part.find('{') { &name_part[..pos] } else { name_part };

                metrics.insert(metric_name.to_string(), value);
            }
        }

        Ok(metrics)
    }
}
