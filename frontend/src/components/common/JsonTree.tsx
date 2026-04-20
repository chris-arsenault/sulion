// Collapsible JSON viewer. Used by FileTab for `.json` / `.ndjson`
// sources. Primitives render inline with a colour per type; objects
// and arrays are expandable and show a `{n keys}` / `[n items]`
// affordance when collapsed.
//
// Kept dependency-free. Not a general-purpose JSON editor — this is
// a read-only browser for payload shapes.

import { useState } from "react";

import { Tooltip } from "../ui";
import "./JsonTree.css";

export function JsonTree({
  value,
  depthLimit = 2,
}: {
  value: unknown;
  /** Auto-expand objects shallower than this; anything deeper starts collapsed. */
  depthLimit?: number;
}) {
  return (
    <div className="jsont">
      <Node value={value} depth={0} depthLimit={depthLimit} path="$" />
    </div>
  );
}

function Node({
  value,
  depth,
  depthLimit,
  path,
}: {
  value: unknown;
  depth: number;
  depthLimit: number;
  path: string;
}) {
  if (value === null) return <span className="jsont__null">null</span>;
  if (typeof value === "boolean") {
    return <span className="jsont__bool">{String(value)}</span>;
  }
  if (typeof value === "number") {
    return <span className="jsont__num">{Number.isFinite(value) ? value : String(value)}</span>;
  }
  if (typeof value === "string") {
    return <StringValue text={value} />;
  }
  if (Array.isArray(value)) {
    return (
      <Collapsible
        summary={<span className="jsont__meta">[{value.length} {value.length === 1 ? "item" : "items"}]</span>}
        defaultOpen={depth < depthLimit}
      >
        <ul className="jsont__list">
          {value.map((v, i) => (
            <li key={`${path}[${i}]`}>
              <span className="jsont__key">{i}:</span>{" "}
              <Node
                value={v}
                depth={depth + 1}
                depthLimit={depthLimit}
                path={`${path}[${i}]`}
              />
            </li>
          ))}
        </ul>
      </Collapsible>
    );
  }
  if (typeof value === "object") {
    const obj = value as Record<string, unknown>;
    const keys = Object.keys(obj);
    return (
      <Collapsible
        summary={<span className="jsont__meta">{`{${keys.length} ${keys.length === 1 ? "key" : "keys"}}`}</span>}
        defaultOpen={depth < depthLimit}
      >
        <ul className="jsont__list">
          {keys.map((k) => (
            <li key={`${path}.${k}`}>
              <span className="jsont__key">{k}:</span>{" "}
              <Node
                value={obj[k]}
                depth={depth + 1}
                depthLimit={depthLimit}
                path={`${path}.${k}`}
              />
            </li>
          ))}
        </ul>
      </Collapsible>
    );
  }
  return <span className="jsont__null">?</span>;
}

const STRING_TRUNCATE = 160;

function StringValue({ text }: { text: string }) {
  const [full, setFull] = useState(false);
  if (text.length <= STRING_TRUNCATE || full) {
    return <span className="jsont__str">{JSON.stringify(text)}</span>;
  }
  return (
    <>
      <span className="jsont__str">
        {JSON.stringify(text.slice(0, STRING_TRUNCATE))}
      </span>
      <Tooltip label={`Show all ${text.length} characters`}>
        <button
          type="button"
          className="jsont__expand"
          onClick={() => setFull(true)}
        >
          …+{text.length - STRING_TRUNCATE}
        </button>
      </Tooltip>
    </>
  );
}

function Collapsible({
  summary,
  defaultOpen,
  children,
}: {
  summary: React.ReactNode;
  defaultOpen: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <span className="jsont__coll">
      <button
        type="button"
        className="jsont__toggle"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <span className={open ? "jsont__chev jsont__chev--open" : "jsont__chev"}>▸</span>
        {summary}
      </button>
      {open && children}
    </span>
  );
}
