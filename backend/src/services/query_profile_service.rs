use crate::models::ProfileListItem;
use crate::services::mysql_client::is_doris_catalogs;
use crate::services::{MySQLClient, StarRocksClient};
use crate::utils::{ApiError, ApiResult};
use serde::Serialize;
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileMetadata {
    pub catalog: Option<String>,
    pub database: Option<String>,
    pub task_type: Option<String>,
}

pub fn parse_profile_metadata(profile: &str) -> ProfileMetadata {
    // Read only the captured Summary, never infer the session catalog from SQL or connection defaults.
    let mut catalog = None;
    let mut database = None;
    let mut task_type = None;
    for line in profile.lines() {
        let line = line.trim();
        if line.starts_with("Execution Summary:")
            || line.starts_with("MergedProfile:")
            || line.starts_with("Execution Profile")
        {
            break;
        }
        let Some((key, value)) = line
            .strip_prefix("- ")
            .and_then(|line| line.split_once(':'))
        else {
            continue;
        };
        if key.trim() == "Sql Statement" {
            break;
        }
        let value = value.trim();
        if value.is_empty() || matches!(value, "N/A" | "NULL") {
            continue;
        }
        match key.trim() {
            "Default Catalog" => catalog = Some(value.to_string()),
            "Default Db" => database = Some(value.to_string()),
            "Task Type" | "Query Type" => task_type = Some(value.to_ascii_uppercase()),
            _ => {},
        }
    }
    ProfileMetadata { catalog, database, task_type }
}

pub struct QueryProfileService {
    pub client: StarRocksClient,
    pub mysql: MySQLClient,
}

fn column_index(columns: &[String], names: &[&str]) -> Option<usize> {
    columns.iter().position(|column| {
        let key: String = column
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect();
        names.iter().any(|name| {
            column.trim().eq_ignore_ascii_case(name)
                || (!key.is_empty() && key.eq_ignore_ascii_case(name))
        })
    })
}

pub fn parse_profile_list(
    columns: &[String],
    rows: Vec<Vec<String>>,
    engine: &str,
) -> ApiResult<Vec<ProfileListItem>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let id_index = column_index(columns, &["QueryId", "ProfileId"]).ok_or_else(|| {
        ApiError::cluster_connection_failed("Profile response is missing Query ID")
    })?;
    let state_index =
        column_index(columns, &["State", "Status", "TaskState", "QueryState", "状态"]).ok_or_else(
            || ApiError::cluster_connection_failed("Profile response is missing query state"),
        )?;
    rows.into_iter()
        .map(|row| {
            let field = |names: &[&str]| -> String {
                column_index(columns, names)
                    .and_then(|index| row.get(index))
                    .filter(|value| value.as_str() != "NULL")
                    .cloned()
                    .unwrap_or_default()
            };
            let query_id = row
                .get(id_index)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    ApiError::cluster_connection_failed("Profile row is missing Query ID")
                })?
                .clone();
            let raw_state = row.get(state_index).cloned().unwrap_or_default();
            let state = match raw_state.to_ascii_uppercase().as_str() {
                "OK" | "EOF" | "FINISHED" | "SUCCESS" => "FINISHED".to_string(),
                "ERR" | "ERROR" | "FAILED" => "FAILED".to_string(),
                _ => raw_state.to_ascii_uppercase(),
            };
            Ok(ProfileListItem {
                query_id,
                state,
                start_time: field(&["StartTime", "开始时间"]),
                end_time: field(&["EndTime", "结束时间"]),
                time: field(&["Time", "ExecutionDuration", "Total", "执行时长"]),
                statement: field(&["Statement", "Sql", "SqlStatement"]),
                catalog: field(&["Catalog", "DefaultCatalog"]),
                database: field(&["Database", "ExecutionDatabase", "DefaultDb", "执行数据库"]),
                user: field(&["User", "QueryUser", "查询用户"]),
                fe_node: field(&["FeNode", "Frontend", "FE节点"]),
                engine: engine.to_string(),
                task_type: field(&["TaskType", "QueryType", "查询类型"]),
            })
        })
        .collect()
}

fn merge_profile_metadata(profiles: &mut Vec<ProfileListItem>, metadata: Vec<ProfileListItem>) {
    let mut indexes: std::collections::HashMap<String, usize> = profiles
        .iter()
        .enumerate()
        .map(|(index, profile)| (profile.query_id.clone(), index))
        .collect();
    for item in metadata {
        if let Some(index) = indexes.get(&item.query_id).copied() {
            let existing = &mut profiles[index];
            if existing.catalog.is_empty() {
                existing.catalog = item.catalog;
            }
            if existing.database.is_empty() {
                existing.database = item.database;
            }
            if existing.task_type.is_empty() {
                existing.task_type = item.task_type;
            }
        } else {
            indexes.insert(item.query_id.clone(), profiles.len());
            profiles.push(item);
        }
    }
    profiles.sort_by(|a, b| {
        b.start_time
            .cmp(&a.start_time)
            .then(a.query_id.cmp(&b.query_id))
    });
}

fn unwrap_doris_response(payload: Value) -> ApiResult<Value> {
    match payload.get("code").and_then(Value::as_i64) {
        Some(0) => payload.get("data").cloned().ok_or_else(|| {
            ApiError::cluster_connection_failed("Doris profile response has no data")
        }),
        Some(401 | 403) => Err(ApiError::forbidden("Doris denied access to this query/profile")),
        _ => Err(ApiError::cluster_connection_failed(format!(
            "Doris profile API failed: {}",
            payload
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("Invalid response")
        ))),
    }
}

fn parse_doris_history(data: Value) -> ApiResult<Vec<ProfileListItem>> {
    let columns = data
        .get("column_names")
        .or_else(|| data.get("columnNames"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::cluster_connection_failed("Doris profile response has no columns")
        })?;
    let columns: Vec<String> = columns
        .iter()
        .map(|column| {
            column
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| ApiError::cluster_connection_failed("Invalid Doris profile column"))
        })
        .collect::<ApiResult<_>>()?;
    let rows = data
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::cluster_connection_failed("Doris profile response has no rows"))?;
    let rows = rows
        .iter()
        .map(|row| {
            row.as_array()
                .ok_or_else(|| ApiError::cluster_connection_failed("Invalid Doris profile row"))
                .map(|row| {
                    row.iter()
                        .map(|value| match value {
                            Value::Null => String::new(),
                            Value::String(value) => value.clone(),
                            _ => value.to_string(),
                        })
                        .collect()
                })
        })
        .collect::<ApiResult<Vec<Vec<String>>>>()?;
    parse_profile_list(&columns, rows, "doris")
}

impl QueryProfileService {
    pub async fn is_doris(&self) -> ApiResult<bool> {
        let (columns, _) = self.mysql.query_raw("SHOW CATALOGS").await?;
        is_doris_catalogs(&columns)
    }

    async fn doris_get(&self, path: &str) -> ApiResult<Value> {
        let response = self
            .client
            .http_client
            .get(format!("{}{}", self.client.get_base_url(), path))
            .query(&[("is_all_node", "true")])
            .basic_auth(
                &self.client.cluster.username,
                Some(&self.client.cluster.password_encrypted),
            )
            .send()
            .await
            .map_err(|error| {
                ApiError::cluster_connection_failed(format!(
                    "Doris profile request failed: {}",
                    error
                ))
            })?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(ApiError::forbidden("Doris denied access to this query/profile"));
        }
        let response = response.error_for_status().map_err(|error| {
            ApiError::cluster_connection_failed(format!("Doris profile HTTP error: {}", error))
        })?;
        let payload = response.json::<Value>().await.map_err(|error| {
            ApiError::cluster_connection_failed(format!(
                "Invalid Doris profile response: {}",
                error
            ))
        })?;
        unwrap_doris_response(payload)
    }

    pub async fn list_profiles(&self) -> ApiResult<Vec<ProfileListItem>> {
        if !self.is_doris().await? {
            let (columns, rows) = self.mysql.query_raw("SHOW PROFILELIST").await?;
            return parse_profile_list(&columns, rows, "starrocks");
        }
        let mut profiles =
            parse_doris_history(self.doris_get("/rest/v2/manager/query/query_info").await?)?;
        // Doris query_info filters out LOAD and omits Default Catalog. Supplement it with
        // permission-checked SQL metadata from the pooled FE. Other FEs remain in query_info;
        // the UI resolves their missing catalog lazily via the authorized profile endpoint.
        // https://doris.apache.org/docs/4.x/query-acceleration/query-profile/
        // SHOW LOAD PROFILE was verified on the staging 4.1.3 server. Older versions may reject it.
        for sql in ["SHOW QUERY PROFILE LIMIT 2000", "SHOW LOAD PROFILE '/'"] {
            match tokio::time::timeout(std::time::Duration::from_secs(5), self.mysql.query_raw(sql))
                .await
            {
                Ok(Ok((columns, rows))) => {
                    if let Ok(metadata) = parse_profile_list(&columns, rows, "doris") {
                        merge_profile_metadata(&mut profiles, metadata);
                    }
                },
                _ => tracing::debug!("Doris SQL profile metadata unavailable on connected FE"),
            }
        }
        // Live queries may not yet have a retained runtime profile. Do not infer completion from absence.
        for query in self.client.get_queries().await? {
            let live = ProfileListItem {
                query_id: query.query_id.clone(),
                start_time: query.start_time.unwrap_or_default(),
                time: format!("{}ms", query.exec_time),
                state: "RUNNING".to_string(),
                statement: query.sql,
                catalog: query.catalog,
                database: query.database,
                user: query.user,
                end_time: String::new(),
                fe_node: query.fe_ip.unwrap_or_default(),
                engine: "doris".to_string(),
                task_type: "QUERY".to_string(),
            };
            if let Some(index) = profiles
                .iter()
                .position(|profile| profile.query_id == query.query_id)
            {
                profiles[index] = live;
            } else {
                profiles.insert(0, live);
            }
        }
        Ok(profiles)
    }

    pub async fn profile_text(&self, query_id: &str) -> ApiResult<String> {
        let content = if self.is_doris().await? {
            let data = self
                .doris_get(&format!("/rest/v2/manager/query/profile/text/{}", query_id))
                .await?;
            data.get("profile")
                .and_then(Value::as_str)
                .or_else(|| data.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            let (_, rows) = self
                .mysql
                .query_raw(&format!("SELECT get_query_profile('{}')", query_id))
                .await?;
            rows.first()
                .and_then(|row| row.first())
                .cloned()
                .unwrap_or_default()
        };
        if content.trim().is_empty() || content.trim() == "NULL" {
            return Err(ApiError::not_found(
                "No retained runtime profile. The original query must have profiling enabled; older profiles may have expired.",
            ));
        }
        Ok(content)
    }
}

/// Accept one read-only SELECT/CTE statement. Keep semicolons inside strings/identifiers intact.
pub fn validate_explain_sql(sql: &str) -> ApiResult<&str> {
    if sql.len() > 131072 {
        return Err(ApiError::invalid_data("SQL is too long"));
    }
    let bytes = sql.as_bytes();
    let mut index = 0;
    let mut ended = false;
    let mut words = Vec::new();
    while index < bytes.len() {
        let ch = bytes[index];
        if ch.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if ch == b'#'
            || (ch == b'-'
                && bytes.get(index + 1) == Some(&b'-')
                && bytes.get(index + 2).is_none_or(u8::is_ascii_whitespace))
        {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if ch == b'/' && bytes.get(index + 1) == Some(&b'*') {
            if bytes.get(index + 2) == Some(&b'!') {
                return Err(ApiError::invalid_data("Executable SQL comments are not supported"));
            }
            index += 2;
            while index + 1 < bytes.len() && &bytes[index..index + 2] != b"*/" {
                index += 1;
            }
            if index + 1 >= bytes.len() {
                return Err(ApiError::invalid_data("Unclosed SQL comment"));
            }
            index += 2;
            continue;
        }
        if ended {
            return Err(ApiError::invalid_data("Enter exactly one SELECT or WITH query"));
        }
        if ch == b';' {
            ended = true;
            index += 1;
            continue;
        }
        if matches!(ch, b'\'' | b'"' | b'`') {
            let quote = ch;
            index += 1;
            let mut closed = false;
            while index < bytes.len() {
                if bytes[index] == b'\\' {
                    if bytes.get(index + 1) == Some(&quote) {
                        return Err(ApiError::invalid_data(
                            "Use doubled quotes instead of backslash-escaped quotes for EXPLAIN",
                        ));
                    }
                    index += 2;
                    continue;
                }
                if bytes[index] == quote {
                    index += 1;
                    if bytes.get(index) == Some(&quote) {
                        index += 1;
                        continue;
                    }
                    closed = true;
                    break;
                }
                index += 1;
            }
            if !closed {
                return Err(ApiError::invalid_data("Unclosed SQL quote"));
            }
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == b'_' {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            words.push(sql[start..index].to_ascii_uppercase());
        } else {
            index += 1;
        }
    }
    if !matches!(words.first().map(String::as_str), Some("SELECT" | "WITH"))
        || !words.iter().any(|word| word == "SELECT")
        || words.iter().any(|word| {
            matches!(
                word.as_str(),
                "INSERT"
                    | "UPDATE"
                    | "DELETE"
                    | "CREATE"
                    | "DROP"
                    | "ALTER"
                    | "TRUNCATE"
                    | "INTO"
                    | "OUTFILE"
            )
        })
    {
        return Err(ApiError::invalid_data("Only a single SELECT or WITH query can be explained"));
    }
    Ok(sql.trim())
}

#[cfg(test)]
mod profile_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn captured_catalog_metadata_never_uses_sql_or_connection_defaults() {
        let real = parse_profile_metadata(include_str!(
            "../../tests/fixtures/profiles/doris-4.1.3-jdbc.txt"
        ));
        assert_eq!(real.catalog.as_deref(), Some("internal"));
        assert_eq!(real.task_type.as_deref(), Some("QUERY"));
        let external = parse_profile_metadata(
            "Summary:\n - Default Catalog: external_iceberg\n - Default Db: gold\n - Task Type: LOAD\n - Sql Statement: select 1\n - Default Catalog: fake_from_sql\nExecution Summary:",
        );
        assert_eq!(external.catalog.as_deref(), Some("external_iceberg"));
        assert_eq!(external.database.as_deref(), Some("gold"));
        assert_eq!(external.task_type.as_deref(), Some("LOAD"));
        let missing =
            parse_profile_metadata("Summary:\n - Sql Statement: select * from external.db.t");
        assert!(missing.catalog.is_none());
        assert!(
            parse_profile_metadata("Summary:\n - Sql Statement:\n - Default Catalog: fake_sql")
                .catalog
                .is_none()
        );
        assert!(
            parse_profile_metadata("Summary:\n - Default Catalog: N/A")
                .catalog
                .is_none()
        );
    }

    #[test]
    fn sql_metadata_adds_load_and_catalog_without_overwriting_live_state() {
        let mut profiles = parse_doris_history(json!({
            "columnNames": ["Query ID", "Sql", "状态", "查询类型"],
            "rows": [["q1", "select 1", "RUNNING", "QUERY"]]
        }))
        .unwrap();
        let columns = [
            "Profile ID",
            "Task State",
            "Default Catalog",
            "Default Db",
            "Task Type",
            "Sql Statement",
        ]
        .map(str::to_string);
        let rows = vec![
            vec!["q1", "OK", "internal", "db", "QUERY", "select 1"],
            vec!["q2", "OK", "iceberg", "gold", "LOAD", "insert into t select 1"],
            vec!["q2", "OK", "iceberg", "gold", "LOAD", "insert into t select 1"],
        ]
        .into_iter()
        .map(|r| r.into_iter().map(str::to_string).collect())
        .collect();
        merge_profile_metadata(&mut profiles, parse_profile_list(&columns, rows, "doris").unwrap());
        assert_eq!(profiles.len(), 2);
        let query = profiles.iter().find(|p| p.query_id == "q1").unwrap();
        assert_eq!(query.state, "RUNNING");
        assert_eq!(query.catalog, "internal");
        let load = profiles.iter().find(|p| p.query_id == "q2").unwrap();
        assert_eq!(load.task_type, "LOAD");
        assert_eq!(load.catalog, "iceberg");
        assert_eq!(load.state, "FINISHED");
    }

    #[test]
    fn doris_history_maps_columns_and_keeps_failures() {
        let profiles = parse_doris_history(json!({"column_names": ["Status", "Sql", "Query ID", "Execution Duration", "Query User"], "rows": [["EOF", "select 1", "q1", "4ms", "analyst"], ["ERR", "select missing", "q2", "1ms", "analyst"], ["CANCELLED", "select sleep(30)", "q3", "2s", "analyst"]]})).unwrap();
        assert_eq!(profiles.len(), 3);
        assert_eq!(profiles[0].state, "FINISHED");
        assert_eq!(profiles[1].state, "FAILED");
        assert_eq!(profiles[2].state, "CANCELLED");
        assert_eq!(profiles[0].statement, "select 1");
    }

    #[test]
    fn doris_413_localized_headers_and_ok_state() {
        let profiles = parse_doris_history(json!({
            "columnNames": ["Query ID", "FE节点", "查询用户", "执行数据库", "Sql", "查询类型", "开始时间", "结束时间", "执行时长", "状态"],
            "rows": [["q1", "fe:8030", "reader", "db", "SELECT 1", "Query", "start", "end", "12ms", "OK"]]
        })).unwrap();
        assert_eq!(profiles[0].state, "FINISHED");
        assert_eq!(profiles[0].user, "reader");
        assert_eq!(profiles[0].database, "db");
        assert_eq!(profiles[0].fe_node, "fe:8030");
        assert_eq!(profiles[0].time, "12ms");
        assert_eq!(profiles[0].start_time, "start");
        assert_eq!(profiles[0].end_time, "end");
    }

    #[test]
    fn starrocks_aborted_profile_remains_visible() {
        let columns = ["QueryId", "StartTime", "Time", "State", "Statement"].map(str::to_string);
        let profiles = parse_profile_list(
            &columns,
            vec![
                vec!["q1", "now", "1s", "Aborted", "select 1"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            ],
            "starrocks",
        )
        .unwrap();
        assert_eq!(profiles[0].state, "ABORTED");
    }

    #[test]
    fn api_errors_are_not_empty_profile_lists() {
        assert!(unwrap_doris_response(json!({"code": 403, "data": []})).is_err());
        assert!(unwrap_doris_response(json!({"code": 0})).is_err());
        assert!(parse_doris_history(json!({"rows": []})).is_err());
    }

    #[test]
    fn explain_accepts_quoted_semicolons_and_comments() {
        for sql in [
            "SELECT ';' AS value;",
            "-- test\nWITH c AS (SELECT 1) SELECT * FROM c;",
            "SELECT `semi;colon` FROM t",
            "SELECT 'it''s;ok' /* comment; */; -- end",
        ] {
            assert!(validate_explain_sql(sql).is_ok(), "{sql}");
        }
    }

    #[test]
    fn explain_rejects_execution_and_multiple_statements() {
        for sql in [
            "",
            "SELECT 1; DROP TABLE t",
            "EXPLAIN ANALYZE SELECT 1",
            "INSERT INTO t SELECT 1",
            "WITH c AS (SELECT 1) INSERT INTO t SELECT * FROM c",
            "SELECT 1 INTO OUTFILE '/tmp/x'",
            "SELECT 'unclosed",
            "SELECT 1 /* unclosed",
            "/*! SELECT 1 */",
            "SELECT 1--2; DROP TABLE t",
            "SELECT 'x\\'; DROP TABLE t; -- '",
        ] {
            assert!(validate_explain_sql(sql).is_err(), "{sql}");
        }
    }
}
