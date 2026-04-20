# IDT framework

Record of the visual framework applied in the #37 pass. This is the record, not the contract — the tokens in `frontend/src/styles/tokens.css` are authoritative. If a component diverges, update the component, not this doc.

## Direction

Engineering telemetry. Muted canvas, saturated data, monospace as first-class. Sharp corners, no warmth, no whimsy. The app is a reading surface for poweruser operators on 4K, not a consumer product.

## Axes

Components pick **one** axis for their visual state. Never mix.

- **Semantic** — state-of-the-world: `ok` live/staged, `warn` stale/dirty, `crit` error/dead, `info` utility, `atn` attention/orphan/delegate, `mute` inactive. Every semantic state has `{-bg, -fg, -mark}` triples.
- **Category** — tool family: `create`, `util`, `delegate`, `workflow`, `inspect`, `research`, `other`. Used for tool-type chips / badges, not for state.
- **Accent** — the single interactive axis (`--accent`). Hover, active tab, focus ring, selection shadow — all the same blue.

## Tokens

Dark-only in MVP. Light theme would drop in mechanically via a `[data-theme]` scope.

- **Canvas** — `canvas-0` page, `canvas-1` pane, `canvas-2` panel, `canvas-3` hover, `hairline` 1px separator, `scrim` modal backdrop.
- **Text** — `text-hi` (headings/active), `text` (body), `text-mid` (secondary), `text-lo` (metadata), `text-dis` (disabled).
- **Semantic × 6** as above.
- **Category × 7** as above.

## Typography — IBM Plex Sans + IBM Plex Mono

Loaded via `@fontsource`. One scale, five sizes: `--t-meta 11 / --t-ui 12 / --t-body 13 / --t-head 15 / --t-splash 18`. Weights `400 / 500 / 600`. Tabular numerals are the default for stats; apply `.tabular` or `[data-tabular]`. Uppercase metadata labels get `0.04–0.08em` tracking.

## Density lanes

Four heights. Every row picks one, no orphans.

- `--lane-xs 20` — dense tree rows, compact metadata
- `--lane-sm 28` — default UI (sidebar row, tab, filter chip, button)
- `--lane-md 36` — primary buttons, empty-state CTAs
- `--lane-lg 44` — mobile tap target (auto-applied below 768px)

## Iconography

Lucide (1.5 stroke) + eight custom sigils in `frontend/src/icons/sigils/`. Fixed sizes only: 12 / 14 / 16 / 20. No emoji. The custom sigils cover concepts Lucide doesn't carry cleanly: session live/dead/orphan, repo staleness ring, pty binding, dirty marker, unread, parent-session lineage, jsonl source.

Access via `<Icon name={...} size={14} />` — the barrel in `frontend/src/icons/Icon.tsx`.

## Tooltip tier rule

Each piece of information lives in exactly one tier.

- **Tier 0** — always on-screen via glyph + colour + position. Session live/dead/orphan, git staleness, dirty, unread, error, active tab, tab↔session binding, focused pane.
- **Tier 1** — `<Tooltip>` primitive (`@floating-ui/react`, 150ms hover delay). Full path, absolute timestamp, full commit SHA, tool input preview, branch + ahead/behind, role hint. HTML `title=` attributes are banned app-wide.
- **Tier 2** — context menu or embedded inspection inside the relevant pane. Full tool output, JSON tree, file trace, git log, correlation log. Never the Tier-1 tooltip.

When contributing, ask "which tier is this?" before picking a component.

## Shell

`[rail 36px] [sidebar 260–360px drag-resize] [work area 1fr]`.

- **Rail** is functional — a vertical strip of repo sigils (staleness-ring letter) that scrolls the sidebar to that repo when clicked. It also houses the pin toggle and command palette trigger.
- **Sidebar** is drag-resizable and persisted (`sulion.sidebar.width.v1`); pinned state persists in `sulion.sidebar.pinned.v1`.
- **Work area** keeps its existing two-pane split + tab strip. Terminal pane gets a 2px accent border when focused — xterm swallows keys, the frame tells you it's live.
- **Mobile** (<768px) hides the rail, keeps a hamburger top bar, and makes the sidebar a drawer.

No right-side inspector column. Reading surfaces (timeline TurnDetail, file body, diff hunks) stay embedded in their panes; overlays (ToolHoverCard, ThinkingFlyout, SubagentModal) stay transient.

## Primitives

Live in `frontend/src/components/ui/`. Re-export from `./ui` barrel.

- **Lane** — fixed-height row with `{leading, label, meta, trailing}` slots. Backbone of sidebar, tab strip, context menu, inspector headers.
- **Sigil** — 16×16 icon + tone/category modifier. Tier-0 on-screen status indicator.
- **Stat** — tabular-numeric value with optional label + unit + tone.
- **Panel** — `{header, body, footer}` surface with canvas variants.
- **Tab** — icon-forward tab with binding-sigil slot, accent underline for active state.
- **Tooltip** — Tier-1 primitive.
- **Overlay** — modal or anchored transient frame. Shared header / close / Esc / z-index for ToolHoverCard, ThinkingFlyout, SubagentModal.
- **CommandPalette** — Cmd/Ctrl-K. Single entry for navigation and actions. Keeps the "only three shortcuts" rule (palette, Esc, Enter).

## Motion

- `--dur-0 80ms` — hover, press tint, chevron rotate
- `--dur-1 160ms` — panel slide, drawer
- `--dur-2 240ms` — overlay enter
- `--ease cubic-bezier(.4,0,.2,1)` — standard
- `--pulse 2s infinite` — reserved for stale-red and unread only
- `prefers-reduced-motion` kills `--dur-2` and the pulse

No other motion. A transition that isn't communicating state shouldn't exist.

## Focus

`outline: 2px solid var(--accent); outline-offset: 2px` applied globally via `:focus-visible`. Never removed without replacement. Terminal pane shows an accent border when focused because its child is a canvas that can't take outlines.

## Deferred

Kept explicitly off-ramp so the shape can hold. File a ticket before adding.

- Light theme (palette is OKLCH-ready — mechanical to add)
- Per-user token overrides
- Work-area column reconfiguration beyond the existing horizontal split
- Per-tool-type icons in TurnRow (ship category-level sigils first, add per-tool only if ambiguous at a glance)
- Radial menus
- Keyboard navigation beyond Cmd-K / Esc / Enter
