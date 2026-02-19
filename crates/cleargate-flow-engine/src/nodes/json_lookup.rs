//! Built-in `json_lookup` node handler.
//!
//! A multi-tool node that dispatches on `tool_name` to perform JSON product
//! catalog operations: `search_products`, `check_stock`, and `compare_prices`.
//! Config requires `{"data_file": "path/to/products.json"}`. Expects a
//! top-level JSON array of product objects.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// JSON product catalog lookup node. Dispatches on `tool_name` to perform
/// search_products, check_stock, or compare_prices operations.
pub struct JsonLookupNode;

#[async_trait]
impl NodeHandler for JsonLookupNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "json_lookup".into(),
            label: "JSON Lookup".into(),
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
                    "data_file": { "type": "string", "description": "Path to JSON products file" }
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
            .unwrap_or("search_products")
            .to_string();

        let arguments = inputs.get("arguments").cloned().unwrap_or(json!({}));

        let data_file = config
            .get("data_file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Fatal {
                message: "json_lookup requires 'data_file' in config".into(),
            })?;

        let products = load_products(&PathBuf::from(data_file))?;

        let content = match tool_name.as_str() {
            "search_products" => search_products(&products, &arguments)?,
            "check_stock" => check_stock(&products, &arguments)?,
            "compare_prices" => compare_prices(&products, &arguments)?,
            other => {
                return Err(NodeError::Fatal {
                    message: format!("unknown json_lookup operation: {other}"),
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
// helpers
// ---------------------------------------------------------------------------

fn load_products(path: &PathBuf) -> Result<Vec<Value>, NodeError> {
    let data = std::fs::read_to_string(path).map_err(|e| NodeError::Fatal {
        message: format!("failed to read JSON file: {e}"),
    })?;
    let products: Vec<Value> = serde_json::from_str(&data).map_err(|e| NodeError::Fatal {
        message: format!("failed to parse JSON: {e}"),
    })?;
    Ok(products)
}

// ---------------------------------------------------------------------------
// search_products
// ---------------------------------------------------------------------------

fn search_products(products: &[Value], args: &Value) -> Result<String, NodeError> {
    let query = args.get("query").and_then(|v| v.as_str());
    let category = args.get("category").and_then(|v| v.as_str());
    let max_price = args.get("max_price").and_then(|v| v.as_f64());
    let min_price = args.get("min_price").and_then(|v| v.as_f64());
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    let mut results: Vec<&Value> = Vec::new();

    for product in products {
        // Filter by text query (case-insensitive on name + description).
        if let Some(q) = query {
            let q_lower = q.to_lowercase();
            let name = product.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let desc = product
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !name.to_lowercase().contains(&q_lower) && !desc.to_lowercase().contains(&q_lower) {
                continue;
            }
        }

        // Filter by category.
        if let Some(cat) = category {
            let prod_cat = product
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !prod_cat.eq_ignore_ascii_case(cat) {
                continue;
            }
        }

        // Filter by price range.
        let price = product.get("price").and_then(|v| v.as_f64());
        if let (Some(max), Some(p)) = (max_price, price) {
            if p > max {
                continue;
            }
        }
        if let (Some(min), Some(p)) = (min_price, price) {
            if p < min {
                continue;
            }
        }

        results.push(product);
        if results.len() >= limit {
            break;
        }
    }

    let result = json!({
        "products": results,
        "count": results.len()
    });
    Ok(result.to_string())
}

// ---------------------------------------------------------------------------
// check_stock
// ---------------------------------------------------------------------------

fn check_stock(products: &[Value], args: &Value) -> Result<String, NodeError> {
    // Support single sku or multiple skus.
    let skus: Vec<String> = if let Some(sku) = args.get("sku").and_then(|v| v.as_str()) {
        vec![sku.to_string()]
    } else if let Some(arr) = args.get("skus").and_then(|v| v.as_array()) {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    } else {
        return Err(NodeError::Fatal {
            message: "check_stock requires 'sku' or 'skus' argument".into(),
        });
    };

    let mut stock_info: Vec<Value> = Vec::new();

    for sku in &skus {
        let found = products.iter().find(|p| {
            p.get("sku")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == sku)
        });

        match found {
            Some(product) => {
                let stock = product.get("stock").and_then(|v| v.as_u64()).unwrap_or(0);
                stock_info.push(json!({
                    "sku": sku,
                    "name": product.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                    "stock": stock,
                    "in_stock": stock > 0
                }));
            }
            None => {
                stock_info.push(json!({
                    "sku": sku,
                    "error": "SKU not found"
                }));
            }
        }
    }

    let result = json!({ "stock": stock_info });
    Ok(result.to_string())
}

// ---------------------------------------------------------------------------
// compare_prices
// ---------------------------------------------------------------------------

fn compare_prices(products: &[Value], args: &Value) -> Result<String, NodeError> {
    let skus: Vec<String> = args
        .get("skus")
        .and_then(|v| v.as_array())
        .ok_or_else(|| NodeError::Fatal {
            message: "compare_prices requires 'skus' array argument".into(),
        })?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    let mut items: Vec<Value> = Vec::new();
    let mut prices: Vec<(String, f64)> = Vec::new();

    for sku in &skus {
        let found = products.iter().find(|p| {
            p.get("sku")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == sku)
        });

        match found {
            Some(product) => {
                let price = product.get("price").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let name = product
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                prices.push((sku.clone(), price));
                items.push(json!({
                    "sku": sku,
                    "name": name,
                    "price": price
                }));
            }
            None => {
                items.push(json!({
                    "sku": sku,
                    "error": "SKU not found"
                }));
            }
        }
    }

    let cheapest = prices
        .iter()
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(sku, price)| json!({"sku": sku, "price": price}));

    let most_expensive = prices
        .iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(sku, price)| json!({"sku": sku, "price": price}));

    let result = json!({
        "comparison": items,
        "cheapest": cheapest,
        "most_expensive": most_expensive
    });
    Ok(result.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;

    fn write_test_products(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("products.json");
        let data = json!([
            {
                "sku": "KB-001",
                "name": "Mechanical Keyboard Pro",
                "description": "Cherry MX Blue switches, RGB backlight",
                "category": "Keyboards",
                "price": 89.99,
                "stock": 25,
                "rating": 4.5,
                "tags": ["mechanical", "rgb"]
            },
            {
                "sku": "KB-002",
                "name": "Wireless Keyboard Slim",
                "description": "Bluetooth, low-profile keys",
                "category": "Keyboards",
                "price": 49.99,
                "stock": 0,
                "rating": 4.2,
                "tags": ["wireless", "bluetooth"]
            },
            {
                "sku": "MS-001",
                "name": "Gaming Mouse X",
                "description": "16000 DPI sensor, 8 programmable buttons",
                "category": "Mice",
                "price": 59.99,
                "stock": 15,
                "rating": 4.7,
                "tags": ["gaming", "wired"]
            },
            {
                "sku": "HS-001",
                "name": "Studio Headset",
                "description": "Noise cancelling, 40mm drivers",
                "category": "Headsets",
                "price": 129.99,
                "stock": 8,
                "rating": 4.3,
                "tags": ["noise-cancelling", "wireless"]
            },
            {
                "sku": "CB-001",
                "name": "USB-C Cable 2m",
                "description": "Braided, 100W PD, USB 3.2",
                "category": "Cables & Adapters",
                "price": 12.99,
                "stock": 100,
                "rating": 4.8,
                "tags": ["usb-c", "charging"]
            }
        ]);
        std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap())
            .expect("write test products");
        path
    }

    #[tokio::test]
    async fn test_search_by_query() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_1",
            "tool_name": "search_products",
            "arguments": { "query": "keyboard" }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        assert_eq!(result["role"], "tool");
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["count"], 2);
    }

    #[tokio::test]
    async fn test_search_by_category() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_2",
            "tool_name": "search_products",
            "arguments": { "category": "Mice" }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["count"], 1);
        assert_eq!(content["products"][0]["sku"], "MS-001");
    }

    #[tokio::test]
    async fn test_search_by_price_range() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_3",
            "tool_name": "search_products",
            "arguments": { "max_price": 50.0 }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // KB-002 ($49.99) and CB-001 ($12.99)
        assert_eq!(content["count"], 2);
    }

    #[tokio::test]
    async fn test_search_combined_filters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_combined",
            "tool_name": "search_products",
            "arguments": { "query": "keyboard", "max_price": 60.0 }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // Only KB-002 ($49.99) matches both filters
        assert_eq!(content["count"], 1);
    }

    #[tokio::test]
    async fn test_check_stock_single() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_4",
            "tool_name": "check_stock",
            "arguments": { "sku": "KB-001" }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["stock"][0]["stock"], 25);
        assert_eq!(content["stock"][0]["in_stock"], true);
    }

    #[tokio::test]
    async fn test_check_stock_out_of_stock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_oos",
            "tool_name": "check_stock",
            "arguments": { "sku": "KB-002" }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["stock"][0]["stock"], 0);
        assert_eq!(content["stock"][0]["in_stock"], false);
    }

    #[tokio::test]
    async fn test_check_stock_multi() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_5",
            "tool_name": "check_stock",
            "arguments": { "skus": ["KB-001", "MS-001", "NONEXISTENT"] }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        let stock = content["stock"].as_array().unwrap();
        assert_eq!(stock.len(), 3);
        assert_eq!(stock[0]["in_stock"], true);
        assert_eq!(stock[1]["in_stock"], true);
        assert!(stock[2].get("error").is_some());
    }

    #[tokio::test]
    async fn test_compare_prices() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_6",
            "tool_name": "compare_prices",
            "arguments": { "skus": ["KB-001", "MS-001", "CB-001"] }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        assert_eq!(content["cheapest"]["sku"], "CB-001");
        assert_eq!(content["cheapest"]["price"], 12.99);
        assert_eq!(content["most_expensive"]["sku"], "KB-001");
        assert_eq!(content["most_expensive"]["price"], 89.99);
    }

    #[tokio::test]
    async fn test_missing_data_file() {
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_7",
            "tool_name": "search_products",
            "arguments": {}
        });
        let config = json!({ "data_file": "/nonexistent/products.json" });

        let result = node.run(inputs, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_config() {
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_8",
            "tool_name": "search_products",
            "arguments": {}
        });

        let result = node.run(inputs, &json!({}), &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_check_stock_missing_args() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_9",
            "tool_name": "check_stock",
            "arguments": {}
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unknown_tool_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_10",
            "tool_name": "delete_product",
            "arguments": {}
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_with_min_price() {
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = write_test_products(dir.path());
        let node = JsonLookupNode;
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let inputs = json!({
            "id": "call_min",
            "tool_name": "search_products",
            "arguments": { "min_price": 80.0 }
        });
        let config = json!({ "data_file": json_path.to_str().unwrap() });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        let content: Value = serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
        // KB-001 ($89.99) and HS-001 ($129.99)
        assert_eq!(content["count"], 2);
    }
}
