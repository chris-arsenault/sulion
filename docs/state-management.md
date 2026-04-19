# Frontend state management

Single paradigm: **Zustand for app-wide state**, **component-local
`useState` / `useReducer`** for tab-internal working state, and a
small **typed app command layer** for transient cross-surface actions.
No React context stores.

## Stores

| Store | Purpose |
|---|---|
| `SessionStore` | PTY sessions list, repo list, selected-session URL sync, unread tracking, polling. |
| `RepoStore` | Per-repo git status, file tree state, expansion state, polling. |
| `TabStore` | Thin tab registry only. |
| `ContextMenuStore` | Ephemeral open/close state for the global context-menu layer. |

Each store is a singleton Zustand store with selector-based reads.
Consumers are expected to subscribe to narrow slices or stable actions,
not whole-store objects.

## Transient coordination

Not every cross-surface interaction is durable state. Some flows are
imperative one-shots:

- open a file tab from the sidebar tree
- open a diff tab from a context menu
- close the mobile drawer after a selection
- inject prompt text into the active terminal
- refresh library sections after a ref/prompt mutation

Those use the typed command layer in
`frontend/src/state/AppCommands.ts`, not `window.dispatchEvent(...)`
and not a Zustand store acting like an event queue.

Rule of thumb:

- If the data must be observed over time, survive remounts, or
  describe app structure, it belongs in Zustand.
- If the interaction is a transient "do this now" signal between two
  mounted surfaces, it belongs in the app command layer.

## The tab registry: thin by design

This is still the load-bearing architectural decision.

**Each tab is its own subtree with its own state, analogous to a
separate process in a desktop application.** The registry routes; the
tabs run.

The registry (`TabStore`) holds the minimum needed to know that a tab
exists and where it is:

```ts
interface TabData {
  id: string;
  kind: "terminal" | "timeline" | "file" | "diff" | "search" | "ref" | "prompt";
  sessionId?: string;
  repo?: string;
  path?: string;
  slug?: string;
}
```

Plus pane membership (`panes`) and active-per-pane (`activeByPane`).

The registry does **not** hold:

- Terminal scroll / selection / WebSocket state
- Timeline filters, virtuoso scroll, current-turn selection
- File-tab fetched content, raw-toggle state
- Search query, scope, hit list
- Diff expanded-file set, stage-pending state

Those live inside their owning components and die with the tab's mount
lifecycle unless there is a concrete cross-surface need to promote
them.

## Why Zustand

The stores are shared widely enough that selector-based subscriptions
matter, and the app was already paying the cost of global state
coordination. Zustand gives:

- One state model across tab/session/repo/context-menu domains
- Stable action references without provider plumbing
- Narrow subscriptions so unrelated store updates do not fan out across
  the tree
- Simpler test setup once singleton resets are wired centrally

The thin-registry rule still matters. Zustand is used for subscription
granularity and explicit state ownership, not as permission to hoist
tab-internal state into a global store.

## Promotion rules

Default to local state. Promote data into a store only when at least
one of these is true:

1. Two or more surfaces genuinely need the same state.
2. An outside actor must be able to dispatch into durable state.
3. The state must survive component rearrangement because it describes
   app structure rather than tab internals.

If the need is cross-surface but transient, prefer a typed app command
instead of promoting one-shot signals into a store.

If the only argument is "we might need this elsewhere later", keep it
local.

## Testing

Stores are singleton state, so tests reset them centrally in
`frontend/src/test/setup.ts` before and after each test. Components no
longer need provider wrappers just to resolve store hooks.

The same setup resets the app command layer so one test's emitted
commands cannot leak into the next one.

For the context menu, tests that need the menu DOM should render a
`<ContextMenuHost />` alongside the component under test.
