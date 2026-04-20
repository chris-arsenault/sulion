import { useCallback, useEffect, useMemo, useState } from "react";

import { getRepoFileTrace } from "../api/client";
import type { FileTraceResponse } from "../api/types";
import { useTabs } from "../state/TabStore";
import { buildWorkspaceFileMenuItems } from "./common/fileContextMenu";
import type { MenuItem } from "./common/contextMenuStore";
import {
  contextMenuTriggerProps,
  useContextMenu,
} from "./common/contextMenuStore";

export function FileTracePanel({ repo, path }: { repo: string; path: string }) {
  const [trace, setTrace] = useState<FileTraceResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const openTab = useTabs((store) => store.openTab);
  const openCtx = useContextMenu((store) => store.open);

  useEffect(() => {
    let cancelled = false;
    setTrace(null);
    setError(null);
    getRepoFileTrace(repo, path)
      .then((response) => {
        if (!cancelled) setTrace(response);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "trace load failed");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path, repo]);

  const openTurn = useCallback(
    (touch: FileTraceResponse["touches"][number]) => {
      if (!touch.pty_session_id) return;
      openTab(
        {
          kind: "timeline",
          sessionId: touch.pty_session_id,
          focusTurnId: touch.turn_id,
          focusKey: crypto.randomUUID(),
        },
        "bottom",
      );
    },
    [openTab],
  );

  if (error) {
    return <div className="ft__trace ft__trace--error">trace: {error}</div>;
  }

  if (!trace) {
    return <div className="ft__trace ft__trace--loading">loading trace…</div>;
  }

  return (
    <section className="ft__trace" aria-label="Related timeline turns">
      <div className="ft__trace-header">
        <span className="ft__trace-title">Related timeline turns</span>
        {trace.current_diff && (
          <span className="ft__trace-meta">
            current diff +{trace.current_diff.additions} -{trace.current_diff.deletions}
          </span>
        )}
      </div>
      {trace.touches.length === 0 ? (
        <div className="ft__trace-empty">No projected file touches yet.</div>
      ) : (
        <ul className="ft__trace-list">
          {trace.touches.map((touch) => (
            <TraceRow
              key={`${touch.session_uuid}:${touch.turn_id}:${touch.turn_timestamp}:${touch.touch_kind}`}
              touch={touch}
              repo={repo}
              path={path}
              dirty={trace.dirty}
              openTurn={openTurn}
              openCtx={openCtx}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

type Touch = FileTraceResponse["touches"][number];

function TraceRow({
  touch,
  repo,
  path,
  dirty,
  openTurn,
  openCtx,
}: {
  touch: Touch;
  repo: string;
  path: string;
  dirty: string | null;
  openTurn: (t: Touch) => void;
  openCtx: ReturnType<typeof useContextMenu<(at: { clientX: number; clientY: number }, items: MenuItem[]) => void>>;
}) {
  const sessionTag = touch.session_label?.trim()
    ? touch.session_label
    : touch.pty_session_id?.slice(0, 8) ?? touch.session_uuid.slice(0, 8);
  const canOpenTimeline = Boolean(touch.pty_session_id);

  const buildMenu = useCallback((): MenuItem[] => {
    const items: MenuItem[] = [];
    if (canOpenTimeline) {
      items.push({
        kind: "item",
        id: "open-turn",
        label: "Open turn",
        onSelect: () => openTurn(touch),
      });
      items.push({ kind: "separator" });
    }
    items.push(
      ...buildWorkspaceFileMenuItems({
        repo,
        path,
        dirty,
      }),
    );
    return items;
  }, [canOpenTimeline, openTurn, touch, repo, path, dirty]);

  const { onContextMenu, onKeyDown: onTriggerKey } = useMemo(
    () => contextMenuTriggerProps(openCtx, buildMenu),
    [openCtx, buildMenu],
  );
  const onClick = useCallback(() => openTurn(touch), [openTurn, touch]);

  const body = (
    <div className="ft__trace-body">
      <div className="ft__trace-line">
        <span className="ft__trace-session">{sessionTag}</span>
        <span className="ft__trace-pill">{touch.touch_kind}</span>
        {touch.is_write && <span className="ft__trace-pill">write</span>}
        {touch.operation_type && (
          <span className="ft__trace-op">{touch.operation_type}</span>
        )}
        <span className="ft__trace-time">
          {new Date(touch.turn_timestamp).toLocaleString()}
        </span>
      </div>
      <div className="ft__trace-preview">{touch.turn_preview}</div>
    </div>
  );

  return (
    <li
      className={
        canOpenTimeline
          ? "ft__trace-item"
          : "ft__trace-item ft__trace-item--disabled"
      }
    >
      {canOpenTimeline ? (
        <button
          type="button"
          className="ft__trace-button"
          onClick={onClick}
          onContextMenu={onContextMenu}
          onKeyDown={onTriggerKey}
        >
          {body}
        </button>
      ) : (
        body
      )}
    </li>
  );
}
