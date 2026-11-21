# Conversational planning and web MCP

Working proposal for Sulion plan `d3500094-2392-4533-ae60-2e723e75ffc3`.

## Outcome

A user can develop a plan over multiple conversations and hand it to a fresh
terminal agent by plan ID. The plan carries the requirements and corrections,
agreed decisions and rationale, open questions, hypotheses, examples, and enough
phase detail to continue. Claude.ai and ChatGPT web work on the same
repository-scoped record.
The handoff succeeds without a Markdown export, a transcript, or a live authoring
session. Guidance-list patches and stored implementation detail must work before
agent instructions adopt this workflow.

## One plan, two views

Plans have UUIDs, revisions, scoped listing, phase identities, branch anchors,
inherited guidance, event history, and optional PTY attachments. The CLI and
browser REST routes already share the plan service. An unattached plan is
supported, and `--all-pending` lets the author leave execution phases pending.
`current` and `show` carry full guidance, and every CLI command reminds agents
of the outcome and boundaries. [Published plans](../plans.md) ·
[Plan service](../../backend/src/plans.rs) ·
[REST routes](../../backend/src/api/plan_routes.rs)

The current model loses the relation between guidance and implementation. A
guidance patch replaces an array, and the resulting event captures old and new
text. A phase edit has a general update event with title and status changes,
but the service cannot tell which assumption or decision the phase relied on.
An agent resuming the plan must infer both the intent and the impact of the
correction. [Plan model](../../backend/src/plans/model.rs) ·
[Phase mutations](../../backend/src/plans/phases.rs) ·
[ADR 0014](../adrs/0014-published-plans-and-branch-plans.md)

Use the existing plan as the authoring record. It has an outcome and short
synopsis, a set of individually addressable entries, and its existing phases.
An entry holds a requirement, boundary, principle, assumption, hypothesis,
example, question, or decision. It carries its wording and the rationale or
source needed to understand it. Questions carry what would resolve them and can
link to proposed answers.
Selecting an answer settles the decision and records why it was chosen. Keep
boundary attribution and store existing guidance lists as entries with stable
IDs. Project the active guidance kinds into the lists clients already read.
Keep genuinely shared prose in plan context.

Each phase keeps its ID, short description, and execution state. Store
implementation detail and acceptance on the phase. A phase can name the entries
its design or acceptance relies on, and can name another phase where there is a
real execution dependency. Add these references where they answer what to
revisit after a material change. Use ordinary prose for the rest of the
implementation detail. The sidebar keeps the synopsis and progress view, and
the plan-detail read exposes the authoring record and its applicable inherited
context. The synopsis is an editable introduction to that record.

## Hypotheses and examples

Give both kinds dedicated fields in the full view. They use the same stable IDs,
patches, and history as other authored entries. Their relationship to a goal
determines how an agent uses them.

- **Hypothesis.** Record a possible explanation or approach, the outcome or
  question it serves, and what observation would distinguish it from another
  explanation or show that the approach fails. A hypothesis can stay untested
  while the agent chooses which lead is worth pursuing. When tested, link the
  evidence and record what it supports, contradicts, or leaves inconclusive.
  Keep this judgment separate from acceptance of the outcome. The same
  observation can support both when it directly tests both. A plausible mechanism
  becoming an implementation choice is a separate decision with its own
  rationale. An assumption records a condition the current design relies on and
  may need to revisit.
- **Example.** Keep the concrete scenario or reference, its source, and the
  broader behavior it illustrates. During planning, zoom out from the supplied
  case to the representative feature within the requested scope. Identify the
  role played by the named instance, which parts can vary, and the behavior
  that must hold when they do. Link the example
  to that broader entry and record uncertain interpretations as proposals for
  discussion. Choose acceptance evidence for the broader behavior, using the
  supplied case and other representative cases as appropriate. When the user
  explicitly requires one instance, record that commitment in its own
  requirement and link the example to it.

In the river-refinement discussion, the goal was to stop rivers appearing
everywhere after refinement. Drying and course changes were candidate mechanisms
to investigate. The plan could record whether a test supported a mechanism and
still leave the outcome open until representative refined regions showed that
river placement responds to the surrounding land. In the city hot-spot
discussion, an inn illustrates the role of a seed. The site and template
determine the seed, town development builds city-scale blocks and ways, and
the next refinement round reads the numerical fields they changed. Capture
that causal relationship in the plan and link the original inn example to it.

## Lineage that affects a handoff

Three kinds of lineage carry different information.

- **Revisions record what changed and why.** One authored patch commits a
  revision and an event in the existing plan history. Record the authenticated
  author and client, a reason, a source reference or short relevant quotation
  when available, and before/after values for the changed items. An agent's
  claim about the source and the service's knowledge of the authenticated
  author are distinct. Current state lives in the plan and its entries. History
  gives an editor changed items since a revision and explains prior decisions.
- **Entry lineage preserves meaning.** A clarification that preserves the
  commitment keeps its ID. A change that could make work based on it wrong
  creates a successor and retires the old entry. A resolved question links to
  the decision that answered it. Reopening it records the reason and whether
  the earlier decision still governs work provisionally. Relate replacements
  in either direction so a split or consolidation preserves where the resulting
  scope came from. An entry can also be retired with a reason and no successor.
- **Work lineage preserves scope and evidence.** A phase can cite the entries
  it relies on. A superseded reference surfaces as work to revisit, including
  references from descendants to inherited entries. The editor explicitly
  updates affected acceptance and references. Split or combine unstarted
  phases by recording successors and updating their dependents together. Keep
  the original evidence and status for work that ran. Record the authored
  revision and the revisions of applicable ancestors with new execution evidence
  so later edits cannot change what that evidence established. When scope
  changes after execution starts, record the revised work as a linked follow-up.
  Existing execution branches stay attached to their anchors and read live
  ancestor context. An open branch stays anchored to the phase it was created
  to cover.

Consider a plan with an offline requirement, an open storage question, and a
pending persistence phase. The user chooses SQLite, so the author records a
decision that answers the question and links the persistence phase to it. Later
the user requires browser-only deployment and chooses IndexedDB. One patch adds
the browser requirement, supersedes the SQLite decision, points the question
at its new answer, and revises the persistence phase's implementation and
basis while keeping its ID. Offline acceptance still applies. If a branch
already prototyped SQLite, its evidence survives and the stale decision link
tells the next agent what to revisit. If the phase later splits into storage
and migration, the successor links explain which work moved and dependent
phases can name the prerequisite they need.

## One patch for one coherent change

Use typed operations that address entry and phase IDs. The same patch can
contain several operations, including links to entries created in that patch.
The needed operations follow directly from the user edits the author must make.

| Edit | What the operation preserves |
| --- | --- |
| Add, revise, or move a guidance entry | Other entries, their order, stable identity, and provenance. A move can name a neighboring ID. |
| Retire an entry, with successor IDs when replacing or splitting it | Removed guidance stops appearing as current. Earlier wording and references remain resolvable. |
| Resolve or reopen a question | The question, the decision that answered it, and the reason it is open again. |
| Record a hypothesis result or revise what an example illustrates | The original suggestion or case, its source, the linked goal, and the independent acceptance criteria. |
| Revise a phase's implementation, acceptance, or references | Its identity, summary fields unless explicitly edited, and actual progress. |
| Add, move, or replace pending work | Explicit scope and dependency updates. A split or consolidation composes adds, retirements, and reference updates in one patch. |
| Edit the synopsis or outcome | The affected field. The same patch can revise related entries or work. |

A patch targets one owning plan, checks its expected authoring revision, and
applies all changes in one transaction. Each read returns the revision of the
content it presents. An authoring revision advances on content edits. Retain
the existing general revision for browser refresh and operational updates such
as attachment and status, so opening a terminal cannot stale a wording edit.
Check current execution state for structural edits inside the transaction.
Keep a single current authored revision for each plan. On conflict report the
expected and current revisions. The client can read history since its base
revision to assess and reapply its intent. A rejected patch leaves all items
and events untouched.

An inherited entry is edited on the ancestor that owns it. Other plans see it
live. A descendant reference to superseded intent remains visible for review
until its author updates the descendant. Compare inherited guidance in the
branch's current read with the guidance it relied on when it did the work.
Existing branch creation also marks pending anchors in progress. Give planning
expansions a way to attach their scope to the anchor while leaving execution
pending. Keep operational status changes on their existing paths.

Derive stable IDs for existing guidance on read, preserving order and
attribution. The first native patch materializes those entries. Existing event
history remains readable from before IDs were assigned. Guide
all first-party content writers through the same revision check before enabling
shared authoring, including deliberate whole-list replacement. This closes the
path where an older editor could overwrite a new decision with an entire array.
Readers of the compact plan keep their existing shape.

## Authoring contract

Before responding to a turn that materially changes the plan, read the current
plan and inherited guidance. Work out whether the user has clarified intent,
changed a commitment, suggested a hypothesis, given an example, proposed an
alternative, or asked a question. Commit the resulting authored changes and their
consequences for the work.
When resuming, use the current entries to determine what governs execution and
follow the relevant history when the reason for a choice matters. Creating or
refining a plan confers no permission to execute it.

The CLI exposes targeted commands for plan, phase, and entry fields. MCP
offers the same edits as typed tools. Text fields hold ordinary prose, and
callers target existing entries and phases by Sulion ID. Sulion assigns IDs
when callers add records. Use JSON batches for changes that must commit together.

Provide just-in-time documentation in CLI read and command help, and describe
the same semantics in MCP tool input fields and applicable plan-detail responses.
Keep the guidance with the relevant fields and keep routine progress reminders
compact.

- **Hypothesis** — Link the goal. What observation would support this idea, and
  what would make you set it aside? Record what the test actually established.
- **Example** — What role does the named thing play? What varies, and what
  behavior must hold when it varies? Keep the case and link the generalized
  behavior.

## Guided plan review

Review the plan near the end of drafting, before handing it off for execution.
Repeat the affected part after a substantial patch. A guided review opens the
current authored revision with its outcome, boundaries, and phase details. The
CLI can offer a review read for the whole plan or a selected phase. MCP can
return the same prompts with the corresponding detail read. A small repair may
take one short pass. A larger change walks the phases in delivery order.

1. **Trace the existing path.** Start with the phase's requested behavior. Open
   the governing ADRs and trace a normal entry point through the current owner,
   contracts, data or state, and downstream consumers. Name the existing symbols
   and how the change will reach callers. Identify what the phase can reuse,
   extend, or replace. Check alternate entry points or stored data where they
   depend on the changed contract. If the reviewer lacks source access, record
   the unverified path and what the terminal agent needs to check.
2. **Translate intent into technical design.** Name the design problem found
   along that path. A software pattern describes a recurring design problem, a
   solution structure and its roles, when it applies, and the consequences of
   using it. Start with repository conventions and established patterns. For an
   unresolved design choice, search by the problem, such as translating an
   interface, selecting behavior, maintaining ownership, or committing related
   state changes. Read an applicable catalog entry or primary description and
   check its applicability, trade-offs, and fit with the language and framework.
   The named examples in a conversation can help locate the problem to solve.
   Record the chosen pattern or existing idiom, its concrete participants and
   contracts, why it fits, and its material trade-offs in **Technical details**.
   Put shared choices on the plan and phase-specific choices alongside the work.
   A direct change in the existing owner can have a short explanation. [Object-oriented design patterns](https://www.informit.com/store/design-patterns-elements-of-reusable-object-oriented-9780201633610) ·
   [Enterprise application architecture patterns](https://martinfowler.com/eaaCatalog/index.html)
3. **Review the delivery shape.** For each phase, name the behavior it delivers
   and the evidence that will establish it on the real path. Keep implementation,
   caller wiring, and verification together when they establish one outcome.
   Split where there is a real dependency, decision, migration, or delivery
   boundary. For each proposed abstraction, service, or recovery mechanism,
   identify the current requirement that needs it and check whether the existing
   owner can meet the same requirement more simply. Consolidate phases and remove
   machinery as warranted while preserving the requested behavior.
4. **Reconcile the whole plan.** Update the synopsis after reconciling the
   phases. Read it alongside the current outcome, decisions, and phase order.
   It should explain what will be delivered, how the work fits together, and
   any material open decision. Apply review findings using ordinary revisioned
   patches to the technical details, phases, and synopsis. The reviewed revision
   and existing history are enough to recover the reasoning.

For the Sulion MCP phase, a technical design entry could name the **Adapter**
pattern, then identify the participants. The MCP handler translates transport
input into the same plan mutation used by the CLI and REST. The shared service
owns revision checks, validation, and the transaction. Demonstrate that an MCP
edit is visible through the CLI and that both paths reject the same stale edit.
The choice follows from the requirement that all clients edit one plan under
one set of rules.

## MCP boundary

Use one authenticated Streamable HTTP endpoint limited to Sulion planning.
Expose repository discovery, structured plan and history
reads, plan creation, and the planning edits needed for handoff, including edits
to phases and existing plan trees. Invoke the shared plan service so the same
records, validation, revisions, and history serve the CLI, browser, and MCP.
Record the authenticated actor and connector origin. Keep access revocable
and limited to plan operations across Sulion repositories. The connector's
tools need no database credentials, shell, file, transcript, broker, or PTY access.
The terminal agent attaches by UUID and reads `sulion plan current` before acting.

Recommend a dedicated plans-only HTTPS route for both web clients. Claude.ai
connects from Anthropic's infrastructure and needs internet-reachable HTTPS on
port 443. ChatGPT supports private testing with Secure MCP Tunnel, and its full
web write support depends on an eligible Business, Enterprise, or Edu workspace
and permissions. Verify the actual account and authentication compatibility
before configuring ingress. Public-edge configuration and access to real plan
data require separate authorization under the repository's LAN default and data
publication rules. [Claude connector requirements](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp) ·
[ChatGPT MCP availability](https://help.openai.com/en/articles/12584461-connecting-custom-mcp-servers-in-chatgpt) ·
[Secure MCP Tunnel](https://developers.openai.com/api/docs/guides/secure-mcp-tunnels)

## Milestones

1. **Make the full plan patchable by identity** (`e5b5ec02-03a3-4ff9-9fa6-382a796fb441`).
   Verify one correction can patch a guidance entry and phase implementation
   together. Create and patch hypotheses and examples in the same record as
   requirements, with their own fields and links. Check concurrent editors, an
   intervening PTY attach, legacy records, explicit whole-list replacement, and
   rollback of invalid patches.
2. **Preserve why the plan changed and what it affects** (`7775a276-afaa-4a62-a1a3-e186e2184386`).
   Verify reversal, question reopening, a pending-phase split and consolidation,
   and a descendant with a reference to superseded intent. A fresh terminal can
   recover current instructions, the reason for material changes, open work,
   and still-valid execution evidence by UUID. Verify a hypothesis result can
   be recorded while outcome acceptance is open, and an example retains its
   source when the broader behavior has been refined.
3. **Guide conversational authoring and plan review** (`8f89433c-ca89-4638-8959-7eebc945528b`).
   After the first two milestones, align the shared Codex and Claude
   instructions, just-in-time CLI help, and planning skills. Exercise a dialogue
   that supplies a requirement, hypothesis, and narrow example, then makes and
   revises a decision. A fresh agent can explain what still governs, how it
   would test the hypothesis, and what feature and representative cases the
   example points to. Verify unaffected phase detail and the superseded decision
   remain recoverable.
   Trial the guided review on a small repair and a multi-phase design. The small
   repair stays a single delivery unit. The larger review can find a suitable
   software pattern from a plain-language goal and reconsider it when a user
   offers a different pattern as an example. It grounds the choice in actual
   callers, corrects inappropriate scope or machinery, and brings the synopsis
   into line with the revised plan.
4. **Add a scoped MCP adapter** (`0f7e8e89-2a42-4a2f-8a88-3038c858e3f6`).
   With a local MCP client, create and revise an unattached plan, then read the
   same IDs, revision, and execution details through the CLI. Verify repository
   discovery, authentication, attribution, and conflict behavior at the boundary.
   Expose the same hypothesis, example, and review guidance where MCP clients
   read or patch the relevant plan details.
5. **Verify web-to-terminal handoff** (`419432ae-1137-475d-bc0d-500b3cda9406`).
   Once the user authorizes connector access and any public ingress, use
   synthetic plan content to author and revise a plan in each web client.
   Resume in a fresh Sulion terminal by UUID. The terminal agent can recover the
   current decisions, rationale, remaining questions, and affected phases from
   Sulion itself, including the reviewed technical details, hypothesis evidence,
   and the generalized behavior behind a concrete example.
6. **Retrieve plans by their source turns** (`55cf968d-e6d2-40dc-be06-e74ac3500131`).
   Use the existing audit trail, attachments, and canonical turn IDs as the
   provenance for plan and phase work. Preserve exact change links and mark
   contextual links inferred from timing and attachment. Rank plans ahead of
   transcript hits within the same scope and expose a phase-scoped read with
   audit history. Verify that a search can find a plan using terms that only
   appear in the conversation that led to it.

## Phase 1 execution

Expansion `b9f44f18-ee5a-47bf-8df6-c74c882f3bcc`.

1. Use a revisioned authored document owned by the existing plan row. Typed
   operations address entries and phase IDs and change metadata and detail in
   one transaction. Keep legacy guidance as the compact read projection. Test
   rollback, stale writes, and active versus operational revisions.
2. Route browser and PTY changes through the plan service. Keep the authored
   detail on the selected-plan read. A browser draft retains the revision it
   opened against, including after an ambient refresh. Exercise the existing
   REST and correlation paths with an old plan and a new draft.

## Phase 2 execution

Expansion `b7550c5b-96fb-4c64-81f1-91e68a560648`, step
`479956dd-085e-4c6b-8011-9adae893aee9`.

Exercise supersession and phase retirement against the plan service and its
native entry points. Check that pending branch creation leaves execution
pending, current descendant detail exposes affected work, and the original
evidence still identifies the ancestor revision that governed it.

## Phase 3 execution

Expansion `0a75dd95-efd9-4acd-af09-e40ceca019d9`, step
`d8af465e-22f8-4257-b2c3-7a8567c1de6a`.

Use the existing authoring contract for JIT help in `sulion plan help`
and `sulion plan review`. Keep the installed Codex and Claude instructions
and planning skills compatible with terminals that still run the prior CLI.

The browser editor repair is one delivery unit. `PhaseRow` retains the
phase and revision from form open. `updatePlanPhase` sends that revision
through `patch_plan_phase` to `plans::update_phase`, which checks the base
under the plan lock before writing content or status. A conflict keeps the
form open. The existing form, endpoint and Postgres transaction cover the
change. [Optimistic Offline Lock](https://martinfowler.com/eaaCatalog/optimisticOfflineLock.html)

The wider plan has separate delivery boundaries for the native write contract,
lineage, agent guidance and the connector. `dispatch_control` and
`patch_authoring` already own their transport inputs and share
`plans::apply_patch`. The MCP tool handler adapts its JSON-RPC input to
that contract. The plan service owns revision checks and commits the document,
phase projection and history together. A Facade can provide a simpler view
over several operations. The design here needs an Adapter for each transport
and a single plan service that owns the write. Reuse the SDK for transport,
the existing OAuth verifier for signatures and authority, and the SQL
transaction for the commit.

## Phase 4 execution

Expansion `f359027a-1809-4ca2-bdb4-87fc00360ad0`, step
`c5030cba-0217-4497-bea7-8bdc74c88fdb`.

Mount the plans-only route in the control plane. Verify the dedicated client
and Cognito owner, checking any resource binding against the MCP URL. The isolated
Postgres test creates and revises an unattached draft over Streamable HTTP,
works across repositories, rejects stale edits, reads guided review, and attaches
a fresh terminal to the same UUID. The terminal reads the same revision,
typed entries, and phase details.

The local connector test uses a synthetic issuer and proves the Sulion
handoff path.

## Phase 5 route and authoring

Expansion `30143d98-cd93-4959-8aac-9772650000cc`.

1. The original prose-save experiment
   (`6d284c7c-ace4-420f-b1a3-3b99e8c8cb78`) was superseded by direct edits.
2. Wire the MCP paths through the existing edge
   (`ae592ec3-a096-497a-a03b-da182f9964ff`). Use exact ALB and nginx
   paths on the existing Sulion host, AWS proxy, WireGuard, and TrueNAS
   frontend. Use the existing Terraform, SSM, and Compose path for the
   connector settings.

Expansion `338f6643-5df9-4534-98f5-1dc3da861b55` replaces the prose-save
interface. Verify targeted CLI and MCP updates by stable ID, server-assigned
IDs on create, and revision checks and rollback on optional JSON batches.

Expansion `583c3bb9-0da2-4a35-856c-0a4809b5c40a` follows Tastebase's OAuth
discovery setup. Register Claude's fixed callback on the dedicated client in
the shared Cognito pool and bring up the protected resource and authorization
metadata through the existing edge. Add the exact ChatGPT callback after the
client can discover the endpoint. Discover repositories from Sulion's catalog
and saved plans.
