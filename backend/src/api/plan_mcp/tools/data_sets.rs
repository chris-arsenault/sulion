use super::{arguments, tool};
use crate::{api::node_proxy, node_protocol::NodeRequestKind, AppState};
use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(JsonSchema, Deserialize)]
struct Checkout {
    /// Repository name. Select either a repo or a workspace.
    repo: Option<String>,
    workspace_id: Option<Uuid>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Document {
    #[serde(flatten)]
    checkout: Checkout,
    /// Checkout-relative filename ending in .dataset.json.
    path: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Field {
    /// string, integer, number, boolean, array or object.
    r#type: String,
    description: Option<String>,
    allowed: Option<Vec<Value>>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Schema {
    /// One or more string or integer fields that identify a row.
    keys: Vec<String>,
    fields: BTreeMap<String, Field>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Define {
    #[serde(flatten)]
    doc: Document,
    schema: Schema,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Expected {
    key: JsonObject,
    revision: String,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Fill {
    #[serde(flatten)]
    doc: Document,
    /// Each object contains the row key and only the fields to change. Null clears a field.
    items: Vec<JsonObject>,
    /// Optionally reject changes to rows modified since read_data_set.
    expected: Option<Vec<Expected>>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Read {
    #[serde(flatten)]
    doc: Document,
    /// Match these field values, including a full key to read one row.
    r#where: Option<JsonObject>,
    /// Return rows missing all these fields.
    missing: Option<Vec<String>>,
    columns: Option<Vec<String>>,
    offset: Option<usize>,
    /// Defaults to 25. Use zero to inspect the schema alone.
    limit: Option<usize>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Import {
    #[serde(flatten)]
    doc: Document,
    /// csv or jsonl.
    format: String,
    /// Relative path of a previously uploaded file, or supply data inline.
    source_path: Option<String>,
    data: Option<String>,
}

#[derive(JsonSchema)]
#[allow(dead_code)]
struct Export {
    #[serde(flatten)]
    read: Read,
    /// csv or jsonl.
    format: String,
    /// Optional new file in the checkout. Omit to return the table inline.
    destination_path: Option<String>,
}

pub(super) fn tools() -> Vec<Tool> {
    vec![
        tool::<Checkout>(
            "list_data_sets",
            "List requirement and inventory documents in the selected checkout.",
            true,
            false,
        ),
        tool::<Define>(
            "define_data_set",
            "Create or revise a repository-owned schema. All existing rows must remain valid.",
            false,
            false,
        ),
        tool::<Fill>(
            "fill_data_set_items",
            "Enumerate keys or patch one or more typed rows. The entire batch commits together.",
            false,
            false,
        ),
        tool::<Read>(
            "read_data_set",
            "Inspect schema, read one row by key, or page selected columns and missing fields.",
            true,
            false,
        ),
        tool::<Import>(
            "import_data_set",
            "Merge CSV or JSONL into a defined dataset by its stable keys.",
            false,
            false,
        ),
        tool::<Export>(
            "export_data_set",
            "Export typed JSONL or spreadsheet CSV. The result includes the schema.",
            false,
            false,
        ),
    ]
}

pub(super) async fn call(
    state: &AppState,
    name: &str,
    params: Option<JsonObject>,
) -> anyhow::Result<Value> {
    let target: Checkout = arguments(params.clone())?;
    let node = match (target.repo, target.workspace_id) {
        (Some(repo), None) if crate::workspace::is_valid_repo_name(&repo) => {
            node_proxy::repo_node(state, &repo).await?
        }
        (None, Some(id)) => node_proxy::workspace_node(state, id).await?,
        _ => anyhow::bail!("select one valid repository or workspace"),
    };
    Ok(node_proxy::request(
        state,
        node,
        NodeRequestKind::DataSet,
        json!({"tool":name, "arguments":params}),
    )
    .await?)
}
