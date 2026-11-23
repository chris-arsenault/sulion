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

mod data_sets;
mod numbers;
mod transfers;
use numbers::{resolve_direct_numbers, NumberedPhase};

use crate::{
    auth::AuthenticatedUser,
    db::Pool,
    plans::{
        self, diagrams, CreatePlanInput, DirectEdit, EntryContent, EntryFields, NewPhase,
        PlanActor, PlanEdit, PlanFields, PlanPatch, PlanView,
    },
};

pub(super) struct PlanTools {
    state: std::sync::Arc<crate::AppState>,
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
    #[serde(flatten)]
    target: PlanTarget,
    /// Include the guided design review with the current authored revision.
    #[serde(default)]
    review: bool,
    /// Focus a guided review on one phase while retaining surrounding context.
    #[serde(alias = "phase_id")]
    phase: Option<plans::PhaseReference>,
}

#[derive(Deserialize, JsonSchema)]
struct PlanHistory {
    #[serde(flatten)]
    target: PlanTarget,
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
    #[serde(flatten)]
    target: PlanTarget,
    #[schemars(skip)]
    revision: Option<i64>,
    reason: Option<String>,
    #[serde(flatten)]
    fields: T,
}

#[derive(Deserialize, JsonSchema)]
struct UpdateEntry {
    #[serde(rename = "entry", alias = "id")]
    #[schemars(with = "plans::PhaseReference")]
    id: Uuid,
    #[serde(flatten)]
    #[schemars(with = "EntryFields<plans::PhaseReference>")]
    fields: EntryFields,
}

#[derive(Deserialize, JsonSchema)]
struct MoveBlock {
    #[serde(rename = "number", alias = "id")]
    #[schemars(with = "plans::PhaseReference")]
    id: Uuid,
    /// Target the beginning of the list by omitting after.
    #[schemars(with = "Option<plans::PhaseReference>")]
    after: Option<Uuid>,
}

#[derive(Deserialize, JsonSchema)]
struct RetireBlock {
    #[serde(rename = "number", alias = "id")]
    #[schemars(with = "plans::PhaseReference")]
    id: Uuid,
    #[serde(default)]
    #[schemars(with = "Vec<plans::PhaseReference>")]
    successors: Vec<Uuid>,
}

#[derive(Deserialize, JsonSchema)]
struct RecordEvidence {
    #[serde(rename = "phase", alias = "id")]
    #[schemars(with = "plans::PhaseReference")]
    id: Uuid,
    observation: String,
    reference: Option<String>,
    #[schemars(skip)]
    observed_revision: i64,
    #[serde(default)]
    #[schemars(skip)]
    ancestors: BTreeMap<Uuid, i64>,
}

#[derive(Deserialize, JsonSchema)]
struct BatchEdit {
    #[serde(flatten)]
    target: PlanTarget,
    /// One ordinary plan edit per string, in execution order.
    edits: Vec<String>,
    #[schemars(skip)]
    revision: Option<i64>,
    reason: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct PlanTarget {
    /// Plan title from list_plans.
    #[serde(alias = "plan_id")]
    plan: String,
    /// Repository containing the plan. Supply it when titles overlap.
    repo: Option<String>,
}

impl PlanTarget {
    async fn resolve(&self, pool: &Pool) -> anyhow::Result<Uuid> {
        if self.repo.is_none() {
            if let Ok(id) = self.plan.parse() {
                return Ok(id);
            }
        }
        plans::lookup(pool, self.repo.as_deref(), &self.plan).await
    }
}

impl PlanTools {
    pub(super) fn new(state: std::sync::Arc<crate::AppState>, browser_url: reqwest::Url) -> Self {
        Self {
            pool: state.pool.clone(),
            state,
            browser_url,
        }
    }

    async fn call(&self, request: CallToolRequestParams, subject: &str) -> anyhow::Result<Value> {
        match request.name.as_ref() {
            name @ ("list_data_sets"
            | "define_data_set"
            | "delete_data_set"
            | "fill_data_set_items"
            | "read_data_set"
            | "import_data_set"
            | "export_data_set") => data_sets::call(&self.state, name, request.arguments).await,
            name @ ("begin_file_upload" | "complete_file_upload" | "upload_file") => {
                transfers::call(&self.state, name, request.arguments, subject).await
            }
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
                Ok(
                    json!({"diagrams": diagrams.iter().map(diagrams::Diagram::result).collect::<Vec<_>>()}),
                )
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
                Ok(json!({"diagram": diagram.result()}))
            }
            "list_plans" => {
                let args: ListPlans = arguments(request.arguments)?;
                anyhow::ensure!(
                    crate::workspace::is_valid_repo_name(&args.repo),
                    "invalid repo name"
                );
                let plans =
                    plans::list_for_repo(&self.pool, &args.repo, args.include_closed).await?;
                Ok(
                    json!({"plans": plans.into_iter().enumerate().map(|(i, plan)| json!({
                    "number": i + 1,
                    "id": plan.id, "title": plan.title, "summary": plan.summary, "status": plan.status,
                    "parent_plan_id": plan.parent_plan_id,
                })).collect::<Vec<_>>()}),
                )
            }
            "read_plan" => {
                let args: ReadPlan = arguments(request.arguments)?;
                let plan = plans::get(&self.pool, args.target.resolve(&self.pool).await?).await?;
                if args.review || args.phase.is_some() {
                    let phase = args
                        .phase
                        .map(|reference| match reference {
                            plans::PhaseReference::Id(id) => Ok(id),
                            plans::PhaseReference::Position(n) => plan
                                .phases
                                .iter()
                                .find(|p| p.position == n)
                                .map(|p| p.id)
                                .ok_or_else(|| anyhow::anyhow!("no phase at position {n}")),
                        })
                        .transpose()?;
                    let mut review = plans::review::review(plan, phase)?;
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
                let id = args.target.resolve(&self.pool).await?;
                plans::get(&self.pool, id).await?;
                let events = plans::events(&self.pool, id).await?;
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
                self.patch_plan(request.arguments.unwrap_or_default(), subject)
                    .await
            }
            name => self.edit(name, request.arguments, subject).await,
        }
    }

    async fn edit(
        &self,
        name: &str,
        params: Option<JsonObject>,
        subject: &str,
    ) -> anyhow::Result<Value> {
        let mut input = params.unwrap_or_default();
        let target: PlanTarget = arguments(Some(input.clone()))?;
        let id = target.resolve(&self.pool).await?;
        resolve_direct_numbers(&self.pool, id, name, &mut input).await?;
        let (_, edit) = direct_edit(name, Some(input))?;
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

    async fn patch_plan(&self, input: JsonObject, subject: &str) -> anyhow::Result<Value> {
        let plan = if input.contains_key("operations") {
            anyhow::ensure!(!input.contains_key("edits"), "supply one batch format");
            let target: PlanTarget = arguments(Some(input.clone()))?;
            let patch: PlanPatch<Option<i64>> = arguments(Some(input))?;
            plans::apply_patch(
                &self.pool,
                target.resolve(&self.pool).await?,
                patch,
                &PlanActor::user(),
                "mcp",
                Some(subject),
                None,
            )
            .await?
        } else {
            let args: BatchEdit = arguments(Some(input))?;
            let commands = crate::plan_cli::split_edits(args.edits.iter().map(String::as_str))?;
            plans::apply_commands(
                &self.pool,
                args.target.resolve(&self.pool).await?,
                plans::PlanCommands {
                    commands,
                    reason: args.reason,
                    revision: args.revision,
                },
                &PlanActor::user(),
                "mcp",
                Some(subject),
                None,
            )
            .await?
        };
        Ok(json!({"plan": self.plan_result(plan)}))
    }

    fn plan_result(&self, plan: PlanView) -> Value {
        let mut value = json!(plan);
        self.decorate_plan(&mut value);
        value
    }

    fn decorate_plan(&self, value: &mut Value) {
        trim_attachment_data(value);
        let mut number = 0;
        let mut number_entries = |entries: &mut Value| {
            for entry in entries
                .as_array_mut()
                .into_iter()
                .flatten()
                .filter(|e| e["retired"] != true)
            {
                number += 1;
                entry["number"] = json!(number);
            }
        };
        number_entries(&mut value["authoring"]["entries"]);
        if let Some(inherited) = value
            .get_mut("inherited_authoring")
            .and_then(Value::as_array_mut)
        {
            for ancestor in inherited {
                number_entries(&mut ancestor["entries"]);
            }
        }
        diagrams::add_plan_urls(value, &self.browser_url);
    }
}

impl ServerHandler for PlanTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Select a plan by title. Read its numbered phases and entries with read_plan. Use patch_plan with ordinary edits when they must commit together. For design review, use read_plan with review enabled. For diagrams, list the repository's registered diagrams. For inventories, use list_data_sets or define_data_set in the owning checkout, then fill_data_set_items by key. Use upload_file for a chat attachment, or begin_file_upload and complete_file_upload for a client that can PUT bytes.",
            )
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut tools = vec![
            tool::<Empty>("list_plan_repositories", "List Sulion repositories and repositories with saved plans or registered diagrams.", true, false),
            tool::<RepoDiagrams>("list_repo_arconis_diagrams", "Find reusable Arconis diagrams in a repository, including diagrams without a plan attachment.", true, false),
            tool::<RegisterDiagram>("register_arconis_diagram", "Register a diagram with the workspace and room IDs returned by Arconis save_workspace. Re-registering the same workspace and room updates its title.", false, false),
            tool::<Edit<AttachDiagram>>("attach_arconis_diagram", "Attach the diagram and supply its complete set of phase UUID and object ID pairs. Save the linked objects in Arconis with their read_plan phase URLs.", false, false),
            tool::<Edit<DiagramId>>("detach_arconis_diagram", "Remove a diagram attachment and its links from this plan. The repo registration and Arconis workspace remain available. Remove corresponding backlinks in Arconis.", false, true),
            tool::<ListPlans>("list_plans", "Find the existing plan by repository and title before creating one.", true, false),
            tool::<ReadPlan>("read_plan", "Read the full plan by title, optionally reviewing one numbered phase.", true, false),
            tool::<PlanHistory>("read_plan_history", "Explain changes since an authoring revision, including why an entry was superseded.", true, false),
            tool::<CreatePlan>("create_plan", "Create an unattached plan with pending phases.", false, false),
            tool::<Edit<PlanFields>>("update_plan", "Update the title, synopsis, outcome or technical details.", false, false),
            tool::<Edit<EntryContent<plans::PhaseReference>>>("add_plan_entry", "Add a typed entry. Use entry numbers for links to other entries.", false, false),
            tool::<Edit<UpdateEntry>>("update_plan_entry", "Change supplied fields on a numbered entry.", false, false),
            tool::<Edit<MoveBlock>>("move_plan_entry", "Move an entry after a neighboring entry or to the start.", false, false),
            tool::<Edit<RetireBlock>>("retire_plan_entry", "Retire an entry and optionally link its successors.", false, true),
            tool::<Edit<NewPhaseContent>>("add_plan_phase", "Add a pending phase and let Sulion assign its ID.", false, false),
            tool::<Edit<NumberedPhase>>("update_plan_phase", "Edit the supplied fields on a numbered phase.", false, false),
            tool::<Edit<MoveBlock>>("move_plan_phase", "Move a phase after a neighboring phase or to the start.", false, false),
            tool::<Edit<RetireBlock>>("retire_plan_phase", "Retire an unstarted phase and optionally link its successors.", false, true),
            tool::<Edit<RecordEvidence>>("record_plan_evidence", "Record an observation for a numbered phase against its current plan context.", false, false),
            tool::<BatchEdit>("patch_plan", "Apply one or more edits together. Example: edits=[\"update --summary 'New scope'\", \"entry set 1 --text 'Support offline use'\", \"phase edit 2 --acceptance 'Works offline'\"]. Use entry add/set/move/retire or phase add/edit/move/retire/evidence. Numbers follow the current order after each edit.", false, true),
        ];
        tools.extend(data_sets::tools());
        tools.extend(transfers::tools());
        Ok(ListToolsResult::with_all_items(tools))
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

fn direct_edit(name: &str, input: Option<JsonObject>) -> anyhow::Result<(PlanTarget, DirectEdit)> {
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
) -> anyhow::Result<(PlanTarget, DirectEdit)> {
    let args: Edit<T> = arguments(input)?;
    Ok((
        args.target,
        DirectEdit {
            revision: args.revision,
            reason: args.reason,
            operation: operation(args.fields),
        },
    ))
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
