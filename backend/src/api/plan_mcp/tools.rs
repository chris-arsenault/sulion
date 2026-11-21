use std::collections::BTreeMap;

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, JsonObject,
        ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
        ToolAnnotations,
    },
    service::RequestContext,
    ErrorData, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::Grant;
use crate::{
    db::Pool,
    plans::{
        self, CreatePlanInput, DirectEdit, EntryContent, EntryFields, NewPhase, PhaseFields,
        PlanActor, PlanEdit, PlanFields, PlanPatch, PlanView,
    },
};

pub(super) struct PlanTools {
    pool: Pool,
}

#[derive(Deserialize, JsonSchema)]
struct ListPlans {
    repo: String,
    #[serde(default)]
    include_closed: bool,
}

#[derive(Deserialize, JsonSchema)]
struct ReadPlan {
    plan_id: Uuid,
    /// Include the guided design review with the current authored revision.
    #[serde(default)]
    review: bool,
    /// Focus a guided review on one phase while retaining surrounding context.
    phase_id: Option<Uuid>,
}

#[derive(Deserialize, JsonSchema)]
struct PlanHistory {
    plan_id: Uuid,
    /// Return authored changes newer than this revision.
    after_revision: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
struct CreatePlan {
    repo: String,
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    outcome: String,
    phases: Vec<InitialPhase>,
}

#[derive(Deserialize, JsonSchema)]
struct InitialPhase {
    title: String,
    #[serde(default)]
    description: String,
}

#[derive(Deserialize, JsonSchema)]
struct NewPhaseContent {
    #[serde(flatten)]
    phase: InitialPhase,
    size: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct Edit<T> {
    plan_id: Uuid,
    /// Optional authoring revision from read_plan. A stale revision rejects the edit.
    revision: Option<i64>,
    reason: Option<String>,
    #[serde(flatten)]
    fields: T,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateEntry {
    id: Uuid,
    #[serde(flatten)]
    fields: EntryFields,
}

#[derive(Deserialize, JsonSchema)]
struct MoveBlock {
    id: Uuid,
    /// Target the beginning of the list by omitting after.
    after: Option<Uuid>,
}

#[derive(Deserialize, JsonSchema)]
struct RetireBlock {
    id: Uuid,
    #[serde(default)]
    successors: Vec<Uuid>,
}

#[derive(Deserialize, JsonSchema)]
struct RecordEvidence {
    id: Uuid,
    observation: String,
    reference: Option<String>,
    observed_revision: i64,
    #[serde(default)]
    ancestors: BTreeMap<Uuid, i64>,
}

#[derive(Deserialize, JsonSchema)]
struct BatchEdit {
    plan_id: Uuid,
    #[serde(flatten)]
    patch: PlanPatch,
}

impl PlanTools {
    pub(super) fn new(pool: Pool) -> Self {
        Self { pool }
    }

    async fn view(&self, plan_id: Uuid, grant: &Grant) -> anyhow::Result<PlanView> {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM plans WHERE id = $1 AND repo_name = ANY($2))",
        )
        .bind(plan_id)
        .bind(&grant.repos)
        .fetch_one(&self.pool)
        .await?;
        if !allowed {
            anyhow::bail!("plan not found in connected repositories");
        }
        plans::get(&self.pool, plan_id).await
    }

    async fn call(&self, request: CallToolRequestParams, grant: &Grant) -> anyhow::Result<Value> {
        match request.name.as_ref() {
            "list_plan_repositories" => Ok(json!({"repositories": grant.repos})),
            "list_plans" => {
                let args: ListPlans = arguments(request.arguments)?;
                ensure_repo(&args.repo, grant)?;
                let plans =
                    plans::list_for_repo(&self.pool, &args.repo, args.include_closed).await?;
                Ok(json!({"plans": plans.into_iter().map(|plan| json!({
                    "id": plan.id, "title": plan.title, "summary": plan.summary, "status": plan.status,
                    "parent_plan_id": plan.parent_plan_id,
                })).collect::<Vec<_>>()}))
            }
            "read_plan" => {
                let args: ReadPlan = arguments(request.arguments)?;
                let plan = self.view(args.plan_id, grant).await?;
                if args.review || args.phase_id.is_some() {
                    let mut review = plans::review::review(plan, args.phase_id)?;
                    trim_attachment_data(&mut review["plan"]);
                    Ok(review)
                } else {
                    Ok(json!({"plan": plan_result(plan), "guidance": plans::review::guidance()}))
                }
            }
            "read_plan_history" => {
                let args: PlanHistory = arguments(request.arguments)?;
                self.view(args.plan_id, grant).await?;
                let events = plans::events(&self.pool, args.plan_id).await?;
                Ok(
                    json!({"events": events.into_iter().filter(|event| args.after_revision.is_none_or(|revision| event.authoring_revision.is_some_and(|current| current > revision))).map(|event| {
                    let mut event = json!(event);
                    if let Some(values) = event.as_object_mut() {
                        values.remove("pty_session_id");
                        values.remove("agent_session_uuid");
                    }
                    event
                }).collect::<Vec<_>>() }),
                )
            }
            "create_plan" => {
                let args: CreatePlan = arguments(request.arguments)?;
                ensure_repo(&args.repo, grant)?;
                let plan = plans::create_with_connector(
                    &self.pool,
                    CreatePlanInput {
                        repo_name: args.repo,
                        title: args.title,
                        summary: args.summary,
                        guidance: plans::PlanGuidance {
                            outcome: args.outcome,
                            ..Default::default()
                        },
                        phases: args
                            .phases
                            .into_iter()
                            .map(|phase| NewPhase {
                                title: phase.title,
                                description: phase.description,
                                size: None,
                                status: None,
                            })
                            .collect(),
                        all_pending: true,
                        attach_current_pty: false,
                    },
                    &PlanActor::user(),
                    Some(&grant.subject),
                )
                .await?;
                Ok(json!({"plan": plan_result(plan)}))
            }
            "patch_plan" => {
                let args: BatchEdit = arguments(request.arguments)?;
                let before = self.view(args.plan_id, grant).await?;
                let plan = plans::apply_patch(
                    &self.pool,
                    args.plan_id,
                    args.patch,
                    &PlanActor::user(),
                    "mcp",
                    Some(&grant.subject),
                    Some(&before.repo_name),
                )
                .await?;
                Ok(json!({"plan": plan_result(plan)}))
            }
            name => {
                let (id, edit) = direct_edit(name, request.arguments)?;
                let before = self.view(id, grant).await?;
                let plan = plans::apply_direct(
                    &self.pool,
                    id,
                    edit,
                    &PlanActor::user(),
                    "mcp",
                    Some(&grant.subject),
                    Some(&before.repo_name),
                )
                .await?;
                Ok(json!({"plan": plan_result(plan)}))
            }
        }
    }
}

impl ServerHandler for PlanTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(format!(
                "Author and review Sulion plans. Read the current plan first. Edit the relevant plan, phase or entry fields directly. Use patch_plan for changes that must commit together. {} {}",
                plans::review::HYPOTHESIS, plans::review::EXAMPLE
            ))
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![
            tool::<Empty>("list_plan_repositories", "List repositories connected for planning.", true, false),
            tool::<ListPlans>("list_plans", "Find the existing plan by repository and title before creating one.", true, false),
            tool::<ReadPlan>("read_plan", "Read the full authored plan and revision by UUID. Optionally get a guided plan or phase review.", true, false),
            tool::<PlanHistory>("read_plan_history", "Explain changes since an authoring revision, including why an entry was superseded.", true, false),
            tool::<CreatePlan>("create_plan", "Create an unattached plan with pending phases.", false, false),
            tool::<Edit<PlanFields>>("update_plan", "Update the title, synopsis, outcome or technical details.", false, false),
            tool::<Edit<EntryContent>>("add_plan_entry", "Add a typed entry and let Sulion assign its ID.", false, false),
            tool::<Edit<UpdateEntry>>("update_plan_entry", "Change supplied fields on the entry with this ID.", false, false),
            tool::<Edit<MoveBlock>>("move_plan_entry", "Move an entry after a neighboring entry or to the start.", false, false),
            tool::<Edit<RetireBlock>>("retire_plan_entry", "Retire an entry and optionally link its successors.", false, true),
            tool::<Edit<NewPhaseContent>>("add_plan_phase", "Add a pending phase and let Sulion assign its ID.", false, false),
            tool::<Edit<PhaseFields>>("update_plan_phase", "Edit the supplied phase fields by ID.", false, false),
            tool::<Edit<MoveBlock>>("move_plan_phase", "Move a phase after a neighboring phase or to the start.", false, false),
            tool::<Edit<RetireBlock>>("retire_plan_phase", "Retire an unstarted phase and optionally link its successors.", false, true),
            tool::<Edit<RecordEvidence>>("record_plan_evidence", "Record a phase observation at the revisions it checked.", false, false),
            tool::<BatchEdit>("patch_plan", "Apply a JSON batch of related edits at the supplied authoring revision.", false, true),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let grant = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<Grant>())
            .ok_or_else(|| ErrorData::invalid_request("missing authenticated plan grant", None))?;
        let result = match self.call(request, grant).await {
            Ok(value) => CallToolResult::structured(value),
            Err(error) if error.downcast_ref::<sqlx::Error>().is_some() => {
                tracing::error!(%error, "MCP plan operation failed");
                CallToolResult::error(vec![ContentBlock::text("Plan storage is unavailable")])
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
        };
        Ok(result.into())
    }
}

#[derive(JsonSchema)]
struct Empty {}

fn tool<T: JsonSchema>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    destructive: bool,
) -> Tool {
    let schema = schemars::schema_for!(T).to_value();
    Tool::new(
        name,
        description,
        schema.as_object().cloned().expect("object tool schema"),
    )
    .with_annotations(
        ToolAnnotations::with_title(name.replace('_', " "))
            .read_only(read_only)
            .destructive(destructive),
    )
}

fn arguments<T: DeserializeOwned>(arguments: Option<JsonObject>) -> anyhow::Result<T> {
    Ok(serde_json::from_value(Value::Object(
        arguments.unwrap_or_default(),
    ))?)
}

fn direct_edit(name: &str, input: Option<JsonObject>) -> anyhow::Result<(Uuid, DirectEdit)> {
    match name {
        "update_plan" => decode_edit(input, PlanEdit::EditPlan),
        "add_plan_entry" => decode_edit(input, |entry| PlanEdit::AddEntry { entry }),
        "update_plan_entry" => decode_edit(input, |UpdateEntry { id, fields }| {
            PlanEdit::EditEntry { id, fields }
        }),
        "move_plan_entry" => decode_edit(input, |MoveBlock { id, after }| PlanEdit::MoveEntry {
            id,
            after,
        }),
        "retire_plan_entry" => decode_edit(input, |RetireBlock { id, successors }| {
            PlanEdit::RetireEntry { id, successors }
        }),
        "add_plan_phase" => decode_edit(input, |NewPhaseContent { phase, size }| {
            PlanEdit::AddPhase {
                id: Uuid::new_v4(),
                title: phase.title,
                description: phase.description,
                size,
            }
        }),
        "update_plan_phase" => decode_edit(input, PlanEdit::EditPhase),
        "move_plan_phase" => decode_edit(input, |MoveBlock { id, after }| PlanEdit::MovePhase {
            id,
            after,
        }),
        "retire_plan_phase" => decode_edit(input, |RetireBlock { id, successors }| {
            PlanEdit::RetirePhase { id, successors }
        }),
        "record_plan_evidence" => decode_edit(
            input,
            |RecordEvidence {
                 id,
                 observation,
                 reference,
                 observed_revision,
                 ancestors,
             }| {
                PlanEdit::RecordEvidence {
                    id,
                    observation,
                    reference,
                    revision: observed_revision,
                    ancestors,
                }
            },
        ),
        _ => anyhow::bail!("unknown tool"),
    }
}

fn decode_edit<T: DeserializeOwned>(
    input: Option<JsonObject>,
    operation: impl FnOnce(T) -> PlanEdit,
) -> anyhow::Result<(Uuid, DirectEdit)> {
    let args: Edit<T> = arguments(input)?;
    Ok((
        args.plan_id,
        DirectEdit {
            revision: args.revision,
            reason: args.reason,
            operation: operation(args.fields),
        },
    ))
}

fn ensure_repo(repo: &str, grant: &Grant) -> anyhow::Result<()> {
    anyhow::ensure!(
        grant.repos.iter().any(|name| name == repo),
        "repository is outside the connector's scope"
    );
    Ok(())
}

fn plan_result(plan: PlanView) -> Value {
    let mut value = json!(plan);
    trim_attachment_data(&mut value);
    value
}

fn trim_attachment_data(value: &mut Value) {
    if let Some(values) = value.as_object_mut() {
        for key in [
            "attachments",
            "created_by_pty_id",
            "created_by_agent_session_uuid",
        ] {
            values.remove(key);
        }
    }
}
