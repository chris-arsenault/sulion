// Stable per-tab DOM hosts, adopted by layout slots via appendChild.
//
// TabHost renders every tab's content exactly once, through a portal,
// into a host element that never changes identity for the life of the
// tab. Panes and the peek overlay render empty TabSlot containers and
// *claim* a tab's host; the registry parents the host element into the
// highest-priority claimant. React only reconciles inside the host, so
// moving it between containers never unmounts the content — terminals
// keep their WebSocket and scrollback across pane drags, display-mode
// switches, and peeks. This extends the "terminal DOM lives outside
// React reconciliation" invariant to the whole tab layer.

interface HostClaim {
  token: number;
  container: HTMLElement;
  priority: number;
}

const hosts = new Map<string, HTMLDivElement>();
const claims = new Map<string, HostClaim[]>();
let nextToken = 1;

/** Stable host element for a tab; created on first use. */
export function hostFor(tabId: string): HTMLDivElement {
  let el = hosts.get(tabId);
  if (!el) {
    el = document.createElement("div");
    el.className = "tab-host__content";
    el.dataset.tabHost = tabId;
    hosts.set(tabId, el);
  }
  return el;
}

function place(tabId: string) {
  // Create on demand: a slot may claim before the portal's first render.
  const el = hostFor(tabId);
  const list = claims.get(tabId) ?? [];
  let top: HostClaim | null = null;
  for (const claim of list) {
    // >= so the most recent claim wins ties (a re-opened overlay takes
    // the host back from a stale claim of equal priority).
    if (!top || claim.priority >= top.priority) top = claim;
  }
  if (!top) {
    // Unclaimed hosts sit detached from the document. Portals render
    // into detached nodes without complaint, so the content stays live.
    el.remove();
    return;
  }
  if (el.parentElement !== top.container) top.container.appendChild(el);
}

/** Adopt a tab's host into `container`. Returns the unclaim function.
 * The highest-priority claim holds the host; on release it moves to the
 * next claimant or detaches. */
export function claimHost(
  tabId: string,
  container: HTMLElement,
  priority: number,
): () => void {
  const token = nextToken++;
  claims.set(tabId, [
    ...(claims.get(tabId) ?? []),
    { token, container, priority },
  ]);
  place(tabId);
  return () => {
    claims.set(
      tabId,
      (claims.get(tabId) ?? []).filter((claim) => claim.token !== token),
    );
    place(tabId);
  };
}

/** Ids with a live host element. */
export function registeredHostIds(): string[] {
  return Array.from(hosts.keys());
}

/** Drop a closed tab's host. Called by TabHost when the tab disappears
 * from the store — never from portal cleanup, which StrictMode runs on
 * mounted components. */
export function releaseHost(tabId: string) {
  hosts.get(tabId)?.remove();
  hosts.delete(tabId);
  claims.delete(tabId);
}

export function resetTabHostRegistry() {
  for (const el of hosts.values()) el.remove();
  hosts.clear();
  claims.clear();
}
