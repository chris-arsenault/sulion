import {
  buildInlineCodeDiff,
  type InlineDiffPart,
} from "./inlineCodeDiffModel";

interface Props {
  oldText?: string;
  newText?: string;
  compact?: boolean;
}

export function InlineCodeDiff({ oldText, newText, compact }: Readonly<Props>) {
  const model = buildInlineCodeDiff(oldText ?? "", newText ?? "", { compact: Boolean(compact) });
  if (model.state === "unchanged") {
    return null;
  }
  if (model.state === "whitespace_only") {
    return <div className="tr-idiff__empty">whitespace-only changes omitted</div>;
  }
  return (
    <div className={compact ? "tr-idiff tr-idiff--compact" : "tr-idiff"}>
      {model.rows.map((row, idx) =>
        row.kind === "collapsed" ? (
          <div key={idx} className="tr-idiff__collapsed">
            ... {row.count} unchanged line{row.count === 1 ? "" : "s"} ...
          </div>
        ) : (
          <div key={idx} className={`tr-idiff__row tr-idiff__row--${row.kind}`}>
            <span className="tr-idiff__gutter">{row.oldLine ?? ""}</span>
            <span className="tr-idiff__gutter">{row.newLine ?? ""}</span>
            <span className="tr-idiff__marker">
              {row.kind === "added" ? "+" : row.kind === "removed" ? "-" : " "}
            </span>
            <code className="tr-idiff__content">
              {row.parts.map((part, partIdx) => (
                <Part key={partIdx} part={part} />
              ))}
            </code>
          </div>
        ),
      )}
    </div>
  );
}

function Part({ part }: Readonly<{ part: InlineDiffPart }>) {
  if (part.kind === "same") {
    return <>{part.value}</>;
  }
  return <span className={`tr-idiff__part tr-idiff__part--${part.kind}`}>{part.value}</span>;
}
