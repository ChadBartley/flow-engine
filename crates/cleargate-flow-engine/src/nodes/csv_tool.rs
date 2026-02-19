//! Built-in `csv_tool` node handler.
//!
//! A multi-tool node that dispatches on `tool_name` to perform CSV data
//! operations: `list_columns`, `query`, and `aggregate`. Config requires
//! `{"data_file": "path/to/file.csv"}`. Input comes from the tool_router
//! fan-out.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// CSV data tool node. Dispatches on `tool_name` to perform list_columns,
/// query, or aggregate operations against a CSV file.
pub struct CsvToolNode;

#[async_trait]
impl NodeHandler for CsvToolNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "csv_tool".into(),
            label: "CSV Tool".into(),
            category: "tools".into(),
            inputs: vec![PortDef {
                name: "input".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: None,
            }],
            outputs: vec![PortDef {
                name: "output".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: None,
            }],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "data_file": { "type": "string", "description": "Path to CSV file" }
                },
                "required": ["data_file"]
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, _ctx: &NodeCtx) -> Result<Value, NodeError> {
        let tool_call_id = inputs
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let tool_name = inputs
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("query")
            .to_string();

        let arguments = inputs.get("arguments").cloned().unwrap_or(json!({}));

        let data_file = config
            .get("data_file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Fatal {
                message: "csv_tool requires 'data_file' in config".into(),
            })?;

        let path = PathBuf::from(data_file);

        let content = match tool_name.as_str() {
            "list_columns" => list_columns(&path)?,
            "query" => query(&path, &arguments)?,
            "aggregate" => aggregate(&path, &arguments)?,
            other => {
                return Err(NodeError::Fatal {
                    message: format!("unknown csv_tool operation: {other}"),
                });
            }
        };

        Ok(json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "name": tool_name,
            "content": content
        }))
    }
}

// ---------------------------------------------------------------------------
// list_columns
// ---------------------------------------------------------------------------

fn list_columns(path: &PathBuf) -> Result<String, NodeError> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| NodeError::Fatal {
        message: format!("failed to open CSV: {e}"),
    })?;

    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| NodeError::Fatal {
            message: format!("failed to read CSV headers: {e}"),
        })?
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Read up to 3 sample rows.
    let mut samples: Vec<BTreeMap<String, String>> = Vec::new();
    for result in rdr.records().take(3) {
        let record = result.map_err(|e| NodeError::Fatal {
            message: format!("failed to read CSV record: {e}"),
        })?;
        let mut row = BTreeMap::new();
        for (i, field) in record.iter().enumerate() {
            if let Some(header) = headers.get(i) {
                row.insert(header.clone(), field.to_string());
            }
        }
        samples.push(row);
    }

    let result = json!({
        "columns": headers,
        "sample_rows": samples,
        "row_count": count_rows(path)?
    });

    Ok(result.to_string())
}

fn count_rows(path: &PathBuf) -> Result<usize, NodeError> {
    let mut rdr = csv::Reader::from_path(path).map_err(|e| NodeError::Fatal {
        message: format!("failed to open CSV: {e}"),
    })?;
    Ok(rdr.records().count())
}

// ---------------------------------------------------------------------------
// query
// ---------------------------------------------------------------------------

fn query(path: &PathBuf, args: &Value) -> Result<String, NodeError> {
    let column = args
        .get("column")
        .and_then(|v| v.as_str())
        .ok_or_else(|| NodeError::Fatal {
            message: "query requires 'column' argument".into(),
        })?;

    let operator = args
        .get("operator")
        .and_then(|v| v.as_str())
        .unwrap_or("eq");

    let value = args.get("value").ok_or_else(|| NodeError::Fatal {
        message: "query requires 'value' argument".into(),
    })?;

    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

    let mut rdr = csv::Reader::from_path(path).map_err(|e| NodeError::Fatal {
        message: format!("failed to open CSV: {e}"),
    })?;

    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| NodeError::Fatal {
            message: format!("failed to read CSV headers: {e}"),
        })?
        .iter()
        .map(|s| s.to_string())
        .collect();

    let col_idx = headers
        .iter()
        .position(|h| h == column)
        .ok_or_else(|| NodeError::Fatal {
            message: format!("column '{column}' not found in CSV"),
        })?;

    let value_str = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };

    let mut matches: Vec<BTreeMap<String, String>> = Vec::new();

    for result in rdr.records() {
        let record = result.map_err(|e| NodeError::Fatal {
            message: format!("failed to read CSV record: {e}"),
        })?;

        let field = record.get(col_idx).unwrap_or("");

        let matched = match operator {
            "eq" => field == value_str,
            "contains" => field.to_lowercase().contains(&value_str.to_lowercase()),
            "gt" | "lt" | "gte" | "lte" => match (field.parse::<f64>(), value_str.parse::<f64>()) {
                (Ok(a), Ok(b)) => match operator {
                    "gt" => a > b,
                    "lt" => a < b,
                    "gte" => a >= b,
                    "lte" => a <= b,
                    _ => false,
                },
                _ => false,
            },
            _ => {
                return Err(NodeError::Fatal {
                    message: format!("unknown operator: {operator}"),
                });
            }
        };

        if matched {
            let mut row = BTreeMap::new();
            for (i, f) in record.iter().enumerate() {
                if let Some(header) = headers.get(i) {
                    row.insert(header.clone(), f.to_string());
                }
            }
            matches.push(row);
            if matches.len() >= limit {
                break;
            }
        }
    }

    let result = json!({
        "matches": matches,
        "count": matches.len()
    });

    Ok(result.to_string())
}

// ---------------------------------------------------------------------------
// aggregate
// ---------------------------------------------------------------------------

fn aggregate(path: &PathBuf, args: &Value) -> Result<String, NodeError> {
    let column = args
        .get("column")
        .and_then(|v| v.as_str())
        .ok_or_else(|| NodeError::Fatal {
            message: "aggregate requires 'column' argument".into(),
        })?;

    let operation = args
        .get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| NodeError::Fatal {
            message: "aggregate requires 'operation' argument".into(),
        })?;

    let group_by = args.get("group_by").and_then(|v| v.as_str());

    let mut rdr = csv::Reader::from_path(path).map_err(|e| NodeError::Fatal {
        message: format!("failed to open CSV: {e}"),
    })?;

    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| NodeError::Fatal {
            message: format!("failed to read CSV headers: {e}"),
        })?
        .iter()
        .map(|s| s.to_string())
        .collect();

    let col_idx = headers
        .iter()
        .position(|h| h == column)
        .ok_or_else(|| NodeError::Fatal {
            message: format!("column '{column}' not found in CSV"),
        })?;

    let group_idx = group_by
        .map(|g| {
            headers
                .iter()
                .position(|h| h == g)
                .ok_or_else(|| NodeError::Fatal {
                    message: format!("group_by column '{g}' not found in CSV"),
                })
        })
        .transpose()?;

    // Collect values, optionally grouped.
    let mut groups: BTreeMap<String, Vec<f64>> = BTreeMap::new();

    for result in rdr.records() {
        let record = result.map_err(|e| NodeError::Fatal {
            message: format!("failed to read CSV record: {e}"),
        })?;

        let field = record.get(col_idx).unwrap_or("");
        let group_key = match group_idx {
            Some(gi) => record.get(gi).unwrap_or("unknown").to_string(),
            None => "_all".to_string(),
        };

        if operation == "count" {
            groups.entry(group_key).or_default().push(1.0);
        } else if let Ok(val) = field.parse::<f64>() {
            groups.entry(group_key).or_default().push(val);
        }
    }

    if group_by.is_some() {
        let mut grouped_results: BTreeMap<String, Value> = BTreeMap::new();
        for (key, values) in &groups {
            grouped_results.insert(key.clone(), compute_aggregate(operation, values)?);
        }
        let result = json!({
            "operation": operation,
            "column": column,
            "group_by": group_by,
            "results": grouped_results
        });
        Ok(result.to_string())
    } else {
        let values = groups.get("_all").cloned().unwrap_or_default();
        let agg = compute_aggregate(operation, &values)?;
        let result = json!({
            "operation": operation,
            "column": column,
            "result": agg
        });
        Ok(result.to_string())
    }
}

fn compute_aggregate(operation: &str, values: &[f64]) -> Result<Value, NodeError> {
    if values.is_empty() {
        return Ok(json!(null));
    }

    match operation {
        "sum" => Ok(json!(values.iter().sum::<f64>())),
        "avg" => Ok(json!(values.iter().sum::<f64>() / values.len() as f64)),
        "min" => Ok(json!(values.iter().cloned().fold(f64::INFINITY, f64::min))),
        "max" => Ok(json!(values
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max))),
        "count" => Ok(json!(values.len())),
        other => Err(NodeError::Fatal {
            message: format!("unknown aggregate operation: {other}"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;

    fn write_test_csv(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("test.csv");
        std::fs::write(
            &path,
            "date,product,category,quantity,unit_price,region\n\
             2024-01-15,Widget A,Electronics,10,29.99,North\n\
             2024-02-20,Widget B,Accessories,5,14.99,South\n\
             2024-03-10,Widget A,Electronics,20,29.99,East\n\
             2024-04-05,Gadget C,Software,3,99.99,West\n\
             2024-05-12,Widget B,Accessories,8,14.99,North\n",
        )
        .expect("write test csv");
        path
    }

    #[tokio::test]
    async fn test_list_columns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_1",
            "tool_name": "list_columns",
            "arguments": {}
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        assert_eq!(result["role"], "tool");
        assert_eq!(result["tool_call_id"], "call_1");

        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        let cols = content["columns"].as_array().unwrap();
        assert_eq!(cols.len(), 6);
        assert_eq!(cols[0], "date");
        assert_eq!(content["row_count"], 5);
    }

    #[tokio::test]
    async fn test_query_eq() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_2",
            "tool_name": "query",
            "arguments": { "column": "product", "operator": "eq", "value": "Widget A" }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["count"], 2);
    }

    #[tokio::test]
    async fn test_query_gt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_3",
            "tool_name": "query",
            "arguments": { "column": "unit_price", "operator": "gt", "value": 20.0 }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // Widget A (29.99) x2 + Gadget C (99.99) = 3
        assert_eq!(content["count"], 3);
    }

    #[tokio::test]
    async fn test_query_lt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_lt",
            "tool_name": "query",
            "arguments": { "column": "quantity", "operator": "lt", "value": 8 }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // quantity 5 and 3
        assert_eq!(content["count"], 2);
    }

    #[tokio::test]
    async fn test_query_contains() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_4",
            "tool_name": "query",
            "arguments": { "column": "product", "operator": "contains", "value": "widget" }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // Widget A x2, Widget B x2 = 4
        assert_eq!(content["count"], 4);
    }

    #[tokio::test]
    async fn test_query_gte_lte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        // gte: quantity >= 10 → rows with 10, 20 = 2
        let inputs = json!({
            "id": "call_gte",
            "tool_name": "query",
            "arguments": { "column": "quantity", "operator": "gte", "value": 10 }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });
        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["count"], 2);

        // lte: unit_price <= 14.99 → Widget B x2 = 2
        let inputs = json!({
            "id": "call_lte",
            "tool_name": "query",
            "arguments": { "column": "unit_price", "operator": "lte", "value": 14.99 }
        });
        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["count"], 2);
    }

    #[tokio::test]
    async fn test_aggregate_sum() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_5",
            "tool_name": "aggregate",
            "arguments": { "column": "quantity", "operation": "sum" }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // 10 + 5 + 20 + 3 + 8 = 46
        assert_eq!(content["result"], 46.0);
    }

    #[tokio::test]
    async fn test_aggregate_avg() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_avg",
            "tool_name": "aggregate",
            "arguments": { "column": "quantity", "operation": "avg" }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // 46 / 5 = 9.2
        let avg = content["result"].as_f64().unwrap();
        assert!((avg - 9.2).abs() < 0.001);
    }

    #[tokio::test]
    async fn test_aggregate_min_max() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let inputs = json!({
            "id": "call_min",
            "tool_name": "aggregate",
            "arguments": { "column": "unit_price", "operation": "min" }
        });
        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["result"], 14.99);

        let inputs = json!({
            "id": "call_max",
            "tool_name": "aggregate",
            "arguments": { "column": "unit_price", "operation": "max" }
        });
        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["result"], 99.99);
    }

    #[tokio::test]
    async fn test_aggregate_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_count",
            "tool_name": "aggregate",
            "arguments": { "column": "quantity", "operation": "count" }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["result"], 5);
    }

    #[tokio::test]
    async fn test_aggregate_group_by() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_6",
            "tool_name": "aggregate",
            "arguments": { "column": "quantity", "operation": "sum", "group_by": "category" }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        let results = &content["results"];
        // Electronics: 10 + 20 = 30, Accessories: 5 + 8 = 13, Software: 3
        assert_eq!(results["Electronics"], 30.0);
        assert_eq!(results["Accessories"], 13.0);
        assert_eq!(results["Software"], 3.0);
    }

    #[tokio::test]
    async fn test_missing_data_file() {
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_7",
            "tool_name": "list_columns",
            "arguments": {}
        });
        let config = json!({ "data_file": "/nonexistent/file.csv" });

        let result = node.run(inputs, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_config() {
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_8",
            "tool_name": "list_columns",
            "arguments": {}
        });

        let result = node.run(inputs, &json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_query_args() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_9",
            "tool_name": "query",
            "arguments": {}
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unknown_tool_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_10",
            "tool_name": "delete_all",
            "arguments": {}
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_query_with_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv_path = write_test_csv(dir.path());
        let node = CsvToolNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_limit",
            "tool_name": "query",
            "arguments": { "column": "product", "operator": "contains", "value": "widget", "limit": 1 }
        });
        let config = json!({ "data_file": csv_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["count"], 1);
    }
}
