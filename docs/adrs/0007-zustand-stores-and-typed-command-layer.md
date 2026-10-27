# ADR 0007: Zustand Stores, a Typed App Command Layer, and a Thin Tab Registry

## Status

Accepted, 2026-04-19.

## Context

The frontend has several surfaces that read the same data: the sidebar,
the tab strip, two panes of tabs, a command palette, and later the
overview and mobile drawer. In the first week the app-wide stores were
React context providers, and the tab registry had grown fields that
belonged to individual tabs (`searchQuery`, `searchScope`, `title`). Any
`openTab` invalidated every descendant of the provider, and tab-internal
state flowed app-wide because it was sitting in the registry.

Cross-surface one-shot actions ("open this file in a tab", "inject this
prompt into the terminal", "close the drawer") were implemented with
`window.dispatchEvent(new CustomEvent("shuttlecraft:…"))` and matching
`addEventListener` calls scattered through components.

On 2026-04-19 the question of state tooling was settled across four
commits in one day, including one reversal.

## Decision

Three rules, all in `docs/state-management.md`.

**Zustand for app-wide state.** Every shared store (`SessionStore`,
`RepoStore`, `TabStore`, `DisplayStore`, `TimelineControlsStore`,
`TurnDetailStore`, `ContextMenuStore`) is a singleton Zustand store read
through narrow selectors. No React context stores. Tests reset the
singletons centrally in `frontend/src/test/setup.ts`.

**A typed app command layer for transient coordination.**
`frontend/src/state/AppCommands.ts` carries imperative one-shots between
mounted surfaces: open a file or diff tab, reveal a meta-repository, close
the mobile drawer, inject prompt text, refresh library lists. If data must
be observed over time or survive remounts it belongs in a store; if it is a
"do this now" signal it is a command.

**The tab registry is thin.** `TabData` holds identity and routing only
(`id`, `kind`, `sessionId?`, `repo?`, `workspaceId?`, `path?`, `slug?`)
plus pane membership and active-per-pane. Each tab is its own subtree with
its own state, analogous to a separate process; the registry routes, the
tabs run. Promotion into a store needs a concrete cross-surface consumer.
`TabHost` renders each tab once into a stable DOM host that panes claim by
reference, so moving a tab never remounts it.

## Alternatives considered

- **React context stores.** The starting point. 0f4dda4 migrated
  `TabStore` to Zustand as a template because tab state was the
  most-written store and context fan-out hit every descendant. 4395974
  rolled that back the same day, arguing the real problem was a fat
  registry, and rewrote the doc as "Why context, not Zustand". 69f99b2
  then moved every store to Zustand once the registry was thin: the stores
  were still shared widely enough that selector-based subscriptions and
  stable action references mattered, and provider plumbing was pure cost.
  The doc's "Why Zustand" section records that reasoning.
- **A fat tab registry.** Rejected in 4395974: `searchQuery`,
  `searchScope`, and `title` were removed from `TabData`, `SearchTab`
  took ownership of its own state, and the "default scope from focused
  tab" feature that required cross-tab reads was dropped. The thin
  registry is called the load-bearing decision in the doc.
- **`window.dispatchEvent` custom events.** Removed by b800daf, which
  deleted the `shuttlecraft:library-changed`, `shuttlecraft:inject-terminal`,
  and `shuttlecraft:close-drawer` events and their listeners in favour of
  typed commands. The doc names this form explicitly as the one not to
  use.
- **A Zustand store acting as an event queue.** Also named in the doc as
  rejected: one-shot signals promoted into durable state have to be
  cleared by their consumer, leak between tests, and blur the rule for
  what belongs in a store.

## Consequences

- Unrelated store updates do not fan out across the tree; a terminal tab
  does not re-render because the sidebar's session list polled.
- Tab-internal state dies when the tab closes, not when it moves panes or
  is hidden. 0700d6c relied on this to add display modes and the peek
  overlay by moving live DOM instead of remounting terminals.
- New cross-surface needs are argued case by case. `TurnDetailStore`
  exists because three surfaces show the same turn; `TimelineControlsStore`
  exists because filters are meant to be shared and persisted.
- Tests render components without provider wrappers and rely on the
  central reset of stores and the command layer.

## References

- [`../state-management.md`](../state-management.md)
- [`../architecture.md`](../architecture.md) — Frontend shape
- 0f4dda4 — migrate `TabStore` to Zustand as a template
- 4395974 — thin tab registry, Zustand rolled back
- 69f99b2 — all stores on Zustand
- b800daf — typed app command layer replaces window events
- 0700d6c — display modes and tabs rendered by reference
