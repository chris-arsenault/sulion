# Frontend state management

This project runs a small mix of React context and Zustand, on purpose:

| Store | Implementation | Why |
|---|---|---|
| `useTabStore` | Zustand (`persist` middleware) | Hot write path, persisted to localStorage, selectors let child components subscribe to narrow slices |
| `SessionStore` | React context | Has network polling + derived hooks; not pressured yet |
| `RepoStore` | React context | Per-repo polling + lazy tree, not pressured yet |
| `ContextMenuProvider` | React context | Tiny ephemeral state, no write pressure |

## Decision log — 2026-04-19

Investigated Zustand vs. continuing with context providers per ticket
#36. Migrated `TabStore` as the template; the other three stores stay
on context for now.

**Why Zustand for TabStore:**

- Writes are frequent (every tab click, drag, close). Every context
  consumer re-renders on every write under the old design — our
  `TabProvider` wrapped the entire work area, so one `openTab` call
  caused re-renders in all of Sidebar, WorkArea, TabStrip, TabHandle,
  every TabContent — whether they cared about the slice that changed
  or not.
- The persist middleware replaces a hand-rolled
  `useEffect(localStorage.setItem)` plus a corresponding `useEffect`
  hydration dance (~30 lines). `merge` gives us a single place to
  self-heal bad persisted state, which we were already doing inline.
- Selector API: existing consumers call `useTabs()` and still get the
  full state (same re-render behaviour as the old context). New
  consumers that care about render cost call
  `useTabStore((s) => s.panes.top)` and only re-render when that
  specific slice changes. Zero call-site migration cost, real upside
  when we need it.

**Why not migrate SessionStore / RepoStore too (yet):**

- They lean on `useEffect` polling loops and derived hooks
  (`useLastViewed`, `useMediaQuery`). Zustand is fine with those but
  the refactor isn't pressing: neither store writes frequently enough
  to be in the hot path.
- One migration is enough to prove the pattern. Go through the TabStore
  migration in review; if the ergonomics land well, promote
  SessionStore next. If not, revert cheaply.

**Testing implications:**

- Zustand stores are module-level singletons. Tests that mutate the
  store should reset it:
  ```ts
  afterEach(() => {
    useTabStore.setState({
      tabs: {},
      panes: { top: [], bottom: [] },
      activeByPane: { top: null, bottom: null },
      hasAnyTab: false,
    });
  });
  ```
- Current test suite doesn't exercise tab mutations so the reset isn't
  wired yet — add it when the first test that opens a tab lands.

## When to reach for Zustand on a new store

- Write pressure: multiple writes per user interaction, or writes from
  inside render loops.
- A persisted slice that needs self-healing on hydration.
- A store read by many components where you'd otherwise split the
  context into "stable actions" vs "frequently-changing data" to keep
  re-renders tight.

Default to React context otherwise. The provider plumbing is cheap and
the mental model is one fewer library.
