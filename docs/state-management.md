# Frontend state management

Single paradigm: **React context** for app-wide stores, **component-local
`useState`** for everything else. No Zustand, no Redux.

## Stores

| Store | Purpose |
|---|---|
| `SessionStore` | PTY sessions list + selection highlight. Polls `/api/sessions`. |
| `RepoStore` | Per-repo git + file tree state. Polls `/api/repos/:name/git`. |
| `TabStore` | Tab **registry** only (see below). |
| `ContextMenuProvider` | Ephemeral menu state for the open popover. |

Everything else — terminal scroll, timeline filters, tab-internal
state, form values — lives in the component that owns it via
`useState`/`useReducer`.

## The tab registry: thin by design

This is the load-bearing architectural decision.

**Each tab is its own subtree with its own state, analogous to a
separate process in a desktop application.** The registry only routes;
the tabs run.

The registry (`TabStore`) holds the minimum needed to know that a tab
exists and where it is:

```ts
interface TabData {
  id: string;
  kind: "terminal" | "timeline" | "file" | "diff" | "search";
  sessionId?: string;  // for session-bound tabs
  repo?: string;       // for repo-bound tabs
  path?: string;       // for file/diff tabs
}
```

Plus pane membership (`panes: { top: string[], bottom: string[] }`)
and active-per-pane (`activeByPane`).

The registry does **not** hold:

- Terminal scroll / selection / WebSocket state
- Timeline filters, virtuoso scroll, current-turn selection
- File-tab fetched content, raw-toggle state
- Search query, scope, hit list
- Diff expanded-file set, stage-pending state

Those all live inside their respective tab components and die when the
tab unmounts. A closed-and-reopened file tab re-fetches. A
closed-and-reopened search tab starts fresh. This is correct: mount
lifecycle = process lifecycle.

### What this rules out

- **Tab A reading Tab B's internal state.** If the Search tab wanted
  to "default scope to whatever session you're currently viewing", it
  would be reaching across tab boundaries. Instead, the user picks
  scope explicitly.
- **Persisting tab-internal state in the registry.** The registry
  knows *what* a tab is (a search tab), not *what the tab is doing*
  (current query). If a particular tab wants to persist its own
  internal state across mounts, it writes to its own localStorage
  key — that's its business, not the registry's.

## Why context, not Zustand

The registry is thin. When `openTab` or `closeTab` fires, the full
registry state change is small and context consumers are a bounded
set (WorkArea + TabStrip + TabHandle + Sidebar writers). Zustand's
selector-based fine-grained subscriptions would matter more if the
registry were fat — but keeping it thin was the correct fix, not
switching tools.

Uniform context keeps:

- One paradigm for contributors to learn
- Tests wrap with providers consistently
- No extra dependency

## When to write local state vs. a store

Default to local. Promote to a store only when:

1. **Two sibling components genuinely need the same state** (not
   "could be wired through a prop"). A form's value field belongs to
   the form component; a search tab's query belongs to the search
   tab; a session's label belongs to the server and is exposed via
   `SessionStore`.
2. **An outside actor must be able to dispatch an action** (e.g.
   sidebar clicks trigger `openTab`). Stores are good at this.

If the only reason to hoist state is "maybe I'll need it elsewhere
later", keep it local.

## Testing

Each store exposes a `<Provider>` wrapper. Tests compose them:

```tsx
<SessionProvider>
  <RepoProvider>
    <TabProvider>
      <ContextMenuProvider>
        <ComponentUnderTest />
      </ContextMenuProvider>
    </TabProvider>
  </RepoProvider>
</SessionProvider>
```

Each provider starts fresh per test (context state is lexically
scoped to the provider instance). No singleton resets needed.
