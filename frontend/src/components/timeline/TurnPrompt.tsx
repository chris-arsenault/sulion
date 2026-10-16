import { useCallback, useMemo, useState } from "react";
import { Markdown } from "./Markdown";
import type { FileLinkTarget } from "./markdownLinks";

const PREVIEW_CHARACTERS = 240;

export function TurnPrompt({ text, fileTarget }: {
  text: string;
  fileTarget: FileLinkTarget | null;
}) {
  const [expanded, setExpanded] = useState(false);
  const characters = useMemo(() => Array.from(text), [text]);
  const long = characters.length > PREVIEW_CHARACTERS;
  const toggle = useCallback(() => setExpanded((value) => !value), []);

  if (!long) return <Markdown source={text} fileTarget={fileTarget} />;

  return <>
    {expanded ? <Markdown source={text} fileTarget={fileTarget} /> : (
      <span className="td__prompt-preview">
        {characters.slice(0, PREVIEW_CHARACTERS).join("").replace(/\s+/g, " ").trimEnd()}…
      </span>
    )}
    <button type="button" className="td__prompt-toggle" aria-expanded={expanded} onClick={toggle}>
      {expanded ? "Show less" : "Show full prompt"}
    </button>
  </>;
}
