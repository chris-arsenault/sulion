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

use crate::{
    auth::AuthenticatedUser,
    db::Pool,
    plans::{
        self, diagrams, CreatePlanInput, DirectEdit, EntryContent, EntryFields, NewPhase,
        PhaseFields, PlanActor, PlanEdit, PlanFields, PlanPatch, PlanView,
    },
};

pub(super) struct PlanTools {
    pool: Pool,
    browser_url: reqwest::Url,
}

#[derive(Deserialize, JsonSchema)]
struct RepoDiagrams {
    repo: String,
}

#[derive(Deserialize, JsonSchema)]
struct RegisterDiagram {
    repo: String,
    /// Durable workspace UUID returned by Arconis save_workspace or open_workspace.
    workspace_id: Uuid,
    /// Ordinary Arconis room ID returned with the workspace.
    room_id: String,
    title: String,
}

#[derive(Deserialize, JsonSchema)]
struct DiagramId {
    workspace_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
struct AttachDiagram {
    workspace_id: Uuid,
    /// Complete set of links for this diagram on this plan. Use [] to attach without phase links.
    phase_links: Vec<diagrams::DiagramPhaseLink>,
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
    /// Omit for an ordinary field edit. Supply plan.authoring.revision from read_plan to compare against that read.
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
    pub(super) fn new(pool: Pool, browser_url: reqwest::Url) -> Self {
        Self { pool, browser_url }
    }

    async fn call(&self, request: CallToolRequestParams, subject: &str) -> anyhow::Result<Value> {
        match request.name.as_ref() {
            "list_plan_repositories" => {
                let repositories: Vec<String> = sqlx::query_scalar(
                    "SELECT repo_name FROM repo_runtime_state WHERE exists = TRUE \
                     UNION SELECT repo_name FROM plans \
                     UNION SELECT repo_name FROM repo_arconis_diagrams ORDER BY repo_name",
                )
                .fetch_all(&self.pool)
                .await?;
                Ok(json!({"repositories": repositories}))
            }
            "list_repo_arconis_diagrams" => {
                let args: RepoDiagrams = arguments(request.arguments)?;
                let diagrams = diagrams::list(&self.pool, &args.repo).await?;
                Ok(json!({"diagrams": diagrams.iter().map(diagram_result).collect::<Vec<_>>()}))
            }
            "register_arconis_diagram" => {
                let args: RegisterDiagram = arguments(request.arguments)?;
                let diagram = diagrams::register(
                    &self.pool,
                    &args.repo,
                    args.workspace_id,
                    &args.room_id,
                    &args.title,
                )
                .await?;
                Ok(json!({"diagram": diagram_result(&diagram)}))
            }
            "list_plans" => {
                let args: ListPlans = arguments(request.arguments)?;
                anyhow::ensure!(
                    crate::workspace::is_valid_repo_name(&args.repo),
                    "invalid repo name"
                );
                let plans =
                    plans::list_for_repo(&self.pool, &args.repo, args.include_closed).await?;
                Ok(json!({"plans": plans.into_iter().map(|plan| json!({
                    "id": plan.id, "title": plan.title, "summary": plan.summary, "status": plan.status,
                    "parent_plan_id": plan.parent_plan_id,
                })).collect::<Vec<_>>()}))
            }
            "read_plan" => {
                let args: ReadPlan = arguments(request.arguments)?;
                let plan = plans::get(&self.pool, args.plan_id).await?;
                if args.review || args.phase_id.is_some() {
                    let mut review = plans::review::review(plan, args.phase_id)?;
                    self.decorate_plan(&mut review["plan"]);
                    Ok(review)
                } else {
                    Ok(
                        json!({"plan": self.plan_result(plan), "guidance": plans::review::guidance()}),
                    )
                }
            }
            "read_plan_history" => {
                let args: PlanHistory = arguments(request.arguments)?;
                plans::get(&self.pool, args.plan_id).await?;
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
                anyhow::ensure!(
                    crate::workspace::is_valid_repo_name(&args.repo),
                    "invalid repo name"
                );
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
                    Some(subject),
                )
                .await?;
                Ok(json!({"plan": self.plan_result(plan)}))
            }
            "patch_plan" => {
                let args: BatchEdit = arguments(request.arguments)?;
                let plan = plans::apply_patch(
                    &self.pool,
                    args.plan_id,
                    args.patch,
                    &PlanActor::user(),
                    "mcp",
                    Some(subject),
                    None,
                )
                .await?;
                Ok(json!({"plan": self.plan_result(plan)}))
            }
            name => {
                let (id, edit) = direct_edit(name, request.arguments)?;
                let plan = plans::apply_direct(
                    &self.pool,
                    id,
                    edit,
                    &PlanActor::user(),
                    "mcp",
                    Some(subject),
                    None,
                )
                .await?;
                Ok(json!({"plan": self.plan_result(plan)}))
            }
        }
    }

    fn plan_result(&self, plan: PlanView) -> Value {
        let mut value = json!(plan);
        self.decorate_plan(&mut value);
        value
    }

    fn decorate_plan(&self, value: &mut Value) {
        trim_attachment_data(value);
        let Some(id) = value["id"].as_str() else {
            return;
        };
        let mut url = self.browser_url.clone();
        url.query_pairs_mut().append_pair("plan", id);
        value["url"] = json!(url.as_str());
        if let Some(phases) = value["phases"].as_array_mut() {
            for phase in phases {
                if let Some(id) = phase["id"].as_str() {
                    let mut phase_url = url.clone();
                    phase_url.query_pairs_mut().append_pair("phase", id);
                    phase["url"] = json!(phase_url.as_str());
                }
            }
        }
    }
}

impl ServerHandler for PlanTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Read the existing plan with read_plan. Use direct tools to edit only the affected fields by UUID. Direct edits can omit revision. Use patch_plan for changes that must commit together and supply plan.authoring.revision from read_plan. For a design review, use read_plan with review enabled. To reuse an Arconis diagram, list the repo's registered diagrams and open its workspace in Arconis.",
            )
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(vec![
            tool::<Empty>("list_plan_repositories", "List Sulion repositories and repositories with saved plans or registered diagrams.", true, false),
            tool::<RepoDiagrams>("list_repo_arconis_diagrams", "Find reusable Arconis diagrams in a repository, including diagrams without a plan attachment.", true, false),
            tool::<RegisterDiagram>("register_arconis_diagram", "Register a diagram with the workspace and room IDs returned by Arconis save_workspace. Re-registering the same workspace and room updates its title.", false, false),
            tool::<Edit<AttachDiagram>>("attach_arconis_diagram", "Attach the diagram and supply its complete set of phase UUID and object ID pairs. Save the linked objects in Arconis with their read_plan phase URLs. When replacing links from an earlier read, pass its plan.authoring.revision.", false, false),
            tool::<Edit<DiagramId>>("detach_arconis_diagram", "Remove a diagram attachment and its links from this plan. The repo registration and Arconis workspace remain available. Remove corresponding backlinks in Arconis.", false, true),
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
            tool::<Edit<PhaseFields>>("update_plan_phase", "Edit the supplied phase fields using plan_id and the phase UUID as id. Omit revision for ordinary edits.", false, false),
            tool::<Edit<MoveBlock>>("move_plan_phase", "Move a phase after a neighboring phase or to the start.", false, false),
            tool::<Edit<RetireBlock>>("retire_plan_phase", "Retire an unstarted phase and optionally link its successors.", false, true),
            tool::<Edit<RecordEvidence>>("record_plan_evidence", "Record a phase observation with the authoring revision of the plan and each governing ancestor you checked.", false, false),
            tool::<BatchEdit>("patch_plan", "Commit the related edits together. Supply revision from plan.authoring.revision in read_plan.", false, true),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let user = context
            .extensions
            .get::<axum::http::request::Parts>()
            .and_then(|parts| parts.extensions.get::<AuthenticatedUser>())
            .ok_or_else(|| ErrorData::invalid_request("missing authenticated user", None))?;
        let result = match self.call(request, &user.sub).await {
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
        "attach_arconis_diagram" => decode_edit(
            input,
            |AttachDiagram {
                 workspace_id,
                 phase_links,
             }| PlanEdit::AttachDiagram {
                workspace_id,
                phase_links,
            },
        ),
        "detach_arconis_diagram" => decode_edit(input, |DiagramId { workspace_id }| {
            PlanEdit::DetachDiagram { workspace_id }
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

fn diagram_result(diagram: &diagrams::Diagram) -> Value {
    json!({
        "workspace_id": diagram.workspace_id,
        "room_id": diagram.room_id,
        "title": diagram.title,
        "url": diagram.url(),
    })
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
