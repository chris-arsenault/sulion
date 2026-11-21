use serde_json::{json, Value};
use uuid::Uuid;

use super::PlanView;

pub const HYPOTHESIS: &str = "Link a testable idea to the outcome it serves. Name an observation that would support it or make you set it aside. Record the result separately from outcome acceptance.";
pub const EXAMPLE: &str = "Identify the role of the named thing, what varies by case, and what behavior must hold when it varies. Preserve the example and link the generalized behavior.";

pub fn guidance() -> Value {
    json!({"hypothesis": HYPOTHESIS, "example": EXAMPLE})
}

pub fn review(plan: PlanView, phase: Option<Uuid>) -> anyhow::Result<Value> {
    if phase.is_some_and(|id| !plan.phases.iter().any(|item| item.id == id)) {
        anyhow::bail!("phase not found in the selected plan");
    }
    Ok(json!({
        "revision": plan.authoring.as_ref().map(|authored| authored.revision),
        "focus_phase": phase,
        "plan": plan,
        "guidance": guidance(),
        "walkthrough": [
            "Start from the requested behavior. Read the relevant ADR and trace the usual entry point, owner, contracts, state and downstream callers. Record what this phase reuses, extends or replaces. Name unverified paths when sources are unavailable.",
            "Name the design problem and search for patterns by what they solve. Read their intent, participants, applicability and consequences, then map the fitting pattern or existing idiom to real symbols and contracts. Record the choice, reason and material trade-off in Technical details.",
            "Group implementation, wiring and verification around one deliverable. Split at a real dependency, decision, migration or delivery boundary. Check each proposed mechanism against the present requirement and the simpler design the existing owner can support.",
            "After any authorized patches, read the current summary, decisions and delivery order together. Reconcile the summary with what will ship and which material choices are open."
        ],
        "pattern_references": [
            "https://www.informit.com/store/design-patterns-elements-of-reusable-object-oriented-9780201633610",
            "https://martinfowler.com/eaaCatalog/index.html"
        ]
    }))
}
