use std::collections::{BTreeMap, HashSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{BoundarySource, PlanBoundary, PlanGuidance};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AuthoredPlan {
    #[serde(default)]
    pub technical_details: String,
    #[serde(default)]
    pub entries: Vec<PlanEntry>,
    #[serde(default)]
    pub phases: BTreeMap<Uuid, PhaseDetails>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub diagrams: BTreeMap<Uuid, Vec<super::diagrams::DiagramPhaseLink>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthoredPlanView {
    pub revision: i64,
    #[serde(flatten)]
    pub document: AuthoredPlan,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub needs_review: BTreeMap<Uuid, Vec<Uuid>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub id: Uuid,
    #[serde(flatten)]
    pub content: EntryContent,
    #[serde(default)]
    pub retired: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub successors: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EntryContent {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(flatten)]
    pub kind: EntryKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryKind {
    Requirement,
    Boundary {
        attribution: BoundarySource,
    },
    Principle,
    Assumption,
    Decision {
        #[serde(default)]
        provisional: bool,
    },
    Question {
        #[serde(default)]
        answer: Option<Uuid>,
    },
    /// Link a testable explanation or approach to its goal. Record the test and what it establishes separately from outcome acceptance.
    Hypothesis {
        #[serde(default)]
        goal: Option<Uuid>,
        #[serde(default)]
        test: String,
        #[serde(default)]
        result: Option<HypothesisResult>,
        #[serde(default)]
        evidence: Option<String>,
    },
    /// Preserve a concrete case and extract its general mechanism. Identify what varies and what behavior must hold when it varies.
    Example {
        #[serde(default)]
        illustrates: Option<Uuid>,
        #[serde(default)]
        generalization: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HypothesisResult {
    Supported,
    Contradicted,
    Inconclusive,
    SetAside,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PhaseDetails {
    #[serde(default)]
    pub implementation: String,
    #[serde(default)]
    pub acceptance: String,
    #[serde(default)]
    pub basis: Vec<Uuid>,
    #[serde(default)]
    pub depends_on: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub follows_up: Vec<Uuid>,
    #[serde(default)]
    pub evidence: Vec<PhaseEvidence>,
    #[serde(default)]
    pub successors: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseEvidence {
    pub observation: String,
    pub reference: Option<String>,
    pub revision: i64,
    pub ancestors: BTreeMap<Uuid, i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanPatch {
    /// Authoring revision returned by the plan detail read.
    pub revision: i64,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    pub operations: Vec<PlanEdit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirectEdit {
    pub revision: Option<i64>,
    pub reason: Option<String>,
    pub operation: PlanEdit,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanFields {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub outcome: Option<String>,
    pub technical_details: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PhaseFields {
    pub id: Uuid,
    pub title: Option<String>,
    pub description: Option<String>,
    pub implementation: Option<String>,
    pub acceptance: Option<String>,
    pub basis: Option<Vec<Uuid>>,
    pub depends_on: Option<Vec<Uuid>>,
    pub follows_up: Option<Vec<Uuid>>,
    pub size: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EntryFields {
    pub text: Option<String>,
    pub source: Option<String>,
    pub rationale: Option<String>,
    pub attribution: Option<BoundarySource>,
    pub provisional: Option<bool>,
    pub answer: Option<Uuid>,
    /// Clear the answer and make the question open again.
    #[serde(default)]
    pub reopen: bool,
    pub goal: Option<Uuid>,
    pub test: Option<String>,
    pub result: Option<HypothesisResult>,
    pub evidence: Option<String>,
    pub illustrates: Option<Uuid>,
    pub generalization: Option<String>,
}

impl EntryFields {
    pub fn apply(self, previous: &EntryContent) -> anyhow::Result<EntryContent> {
        let mut entry = serde_json::to_value(previous)?;
        let kind = entry["kind"].as_str().unwrap_or_default().to_owned();
        let allowed: &[&str] = match kind.as_str() {
            "boundary" => &["attribution"],
            "decision" => &["provisional"],
            "question" => &["answer"],
            "hypothesis" => &["goal", "test", "result", "evidence"],
            "example" => &["illustrates", "generalization"],
            _ => &[],
        };
        if self.reopen {
            anyhow::ensure!(
                kind == "question" && self.answer.is_none(),
                "reopen applies to a question without a new answer"
            );
            entry["answer"] = serde_json::Value::Null;
        }
        let fields = serde_json::to_value(self)?;
        for (field, value) in fields
            .as_object()
            .expect("entry fields serialize as an object")
        {
            if field == "reopen" || value.is_null() {
                continue;
            }
            anyhow::ensure!(
                ["text", "source", "rationale"].contains(&field.as_str())
                    || allowed.contains(&field.as_str()),
                "{field} does not apply to a {kind}"
            );
            entry[field] = value.clone();
        }
        Ok(serde_json::from_value(entry)?)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlanEdit {
    EditPlan(PlanFields),
    AddEntry {
        entry: EntryContent,
    },
    EditEntry {
        id: Uuid,
        #[serde(flatten)]
        fields: EntryFields,
    },
    PutEntry {
        id: Uuid,
        entry: EntryContent,
    },
    MoveEntry {
        id: Uuid,
        after: Option<Uuid>,
    },
    RetireEntry {
        id: Uuid,
        #[serde(default)]
        successors: Vec<Uuid>,
    },
    EditPhase(PhaseFields),
    AddPhase {
        id: Uuid,
        title: String,
        #[serde(default)]
        description: String,
        size: Option<String>,
    },
    MovePhase {
        id: Uuid,
        after: Option<Uuid>,
    },
    RetirePhase {
        id: Uuid,
        #[serde(default)]
        successors: Vec<Uuid>,
    },
    AttachDiagram {
        workspace_id: Uuid,
        phase_links: Vec<super::diagrams::DiagramPhaseLink>,
    },
    DetachDiagram {
        workspace_id: Uuid,
    },
    RecordEvidence {
        id: Uuid,
        observation: String,
        reference: Option<String>,
        revision: i64,
        #[serde(default)]
        ancestors: BTreeMap<Uuid, i64>,
    },
}

impl AuthoredPlan {
    pub(super) fn from_guidance(id: Uuid, guidance: &PlanGuidance) -> Self {
        let mut document = Self::default();
        document.replace_guidance(id, guidance);
        document
    }

    pub(super) fn replace_guidance(&mut self, plan_id: Uuid, guidance: &PlanGuidance) {
        let entries = guidance
            .principles
            .iter()
            .map(|text| (EntryKind::Principle, text))
            .chain(
                guidance
                    .assumptions
                    .iter()
                    .map(|text| (EntryKind::Assumption, text)),
            )
            .chain(guidance.boundaries.iter().map(|item| {
                (
                    EntryKind::Boundary {
                        attribution: item.source,
                    },
                    &item.text,
                )
            }));
        let mut previous = std::mem::take(&mut self.entries);
        let mut active = Vec::new();
        for (position, (kind, text)) in entries.enumerate() {
            if let Some(index) = previous.iter().position(|item| {
                !item.retired && item.content.kind == kind && item.content.text == *text
            }) {
                active.push(previous.remove(index));
            } else {
                let key = format!("{position}:{kind:?}:{text}");
                let id = Uuid::new_v5(&plan_id, key.as_bytes());
                active.push(PlanEntry {
                    id: if previous.iter().chain(&active).any(|item| item.id == id) {
                        Uuid::new_v4()
                    } else {
                        id
                    },
                    content: EntryContent {
                        text: text.clone(),
                        kind,
                        source: None,
                        rationale: None,
                    },
                    retired: false,
                    successors: Vec::new(),
                });
            }
        }
        for entry in &mut previous {
            if matches!(
                entry.content.kind,
                EntryKind::Principle | EntryKind::Assumption | EntryKind::Boundary { .. }
            ) {
                entry.retired = true;
            }
        }
        self.entries = active;
        self.entries.extend(previous);
    }

    pub(super) fn guidance(&self, outcome: String) -> PlanGuidance {
        let mut guidance = PlanGuidance {
            outcome,
            ..Default::default()
        };
        for entry in self.entries.iter().filter(|entry| !entry.retired) {
            let text = &entry.content.text;
            match entry.content.kind {
                EntryKind::Principle => guidance.principles.push(text.clone()),
                EntryKind::Assumption => guidance.assumptions.push(text.clone()),
                EntryKind::Boundary { attribution } => guidance.boundaries.push(PlanBoundary {
                    source: attribution,
                    text: text.clone(),
                }),
                _ => (),
            }
        }
        guidance
    }

    pub(super) fn entry_mut(&mut self, id: Uuid) -> anyhow::Result<&mut PlanEntry> {
        self.entries
            .iter_mut()
            .find(|entry| entry.id == id && !entry.retired)
            .ok_or_else(|| anyhow::anyhow!("active entry not found: {id}"))
    }

    pub(super) fn validate_links(
        &self,
        inherited: &HashSet<Uuid>,
        phases: &HashSet<Uuid>,
    ) -> anyhow::Result<()> {
        let entries = self
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect::<HashSet<_>>();
        for entry in &self.entries {
            for successor in &entry.successors {
                if *successor == entry.id || !entries.contains(successor) {
                    anyhow::bail!("invalid successor {successor} for entry {}", entry.id);
                }
            }
            match entry.content.kind {
                EntryKind::Question {
                    answer: Some(answer),
                } if !entry.retired => {
                    if !self.entries.iter().any(|e| {
                        e.id == answer
                            && !e.retired
                            && matches!(e.content.kind, EntryKind::Decision { provisional: false })
                    }) {
                        anyhow::bail!(
                            "question {} needs a settled decision as its answer",
                            entry.id
                        );
                    }
                }
                EntryKind::Hypothesis {
                    goal: Some(reference),
                    ..
                }
                | EntryKind::Example {
                    illustrates: Some(reference),
                    ..
                } if reference == entry.id
                    || (!entries.contains(&reference) && !inherited.contains(&reference)) =>
                {
                    anyhow::bail!("entry {} refers to an unknown goal: {reference}", entry.id);
                }
                _ => (),
            }
        }
        for (id, detail) in &self.phases {
            if !phases.contains(id) {
                anyhow::bail!("phase not found: {id}");
            }
            for reference in &detail.basis {
                if !entries.contains(reference) && !inherited.contains(reference) {
                    anyhow::bail!("phase {id} cites an unknown entry: {reference}");
                }
            }
            for reference in detail
                .depends_on
                .iter()
                .chain(&detail.follows_up)
                .chain(&detail.successors)
            {
                if *id == *reference || !phases.contains(reference) {
                    anyhow::bail!("phase {id} refers to an invalid phase: {reference}");
                }
            }
        }
        Ok(())
    }
}
