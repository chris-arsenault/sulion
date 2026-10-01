import "./planCommands.css";
import { planCommands, record, text, type PlanCommand as Metadata } from "./planCommandData";

export function PlanCommandMetadata({ input }: { input: unknown }) {
  const commands = planCommands(input);
  if (commands.length === 0) return null;
  return (
    <div className="tr-plans" aria-label="Plan command metadata">
      {commands.map((command, index) => <PlanCommand key={index} command={command} />)}
    </div>
  );
}

function PlanCommand({ command }: { command: Metadata }) {
  const from = Array.isArray(command.from) ? command.from.filter((v) => typeof v === "string").join(", ") : undefined;
  const phases = Array.isArray(command.phases) ? command.phases.filter(record) : [];
  return (
    <section className="tr-plan">
      <div className="tr-plan__heading">
        <span className="tr-plan__action">plan · {text(command.action)}</span>
        {text(command.title) && <strong>{text(command.title)}</strong>}
      </div>
      <dl className="tr-plan__fields">
        <Field label="Plan" value={text(command.plan_id)} />
        <Field label="Phase" value={text(command.phase)} />
        <Field label="Requested status" value={text(command.status)} />
        <Field label="From phases" value={from} />
        <Field label="Size" value={text(command.size)} />
        <Field label="Summary" value={text(command.summary)} />
        <Field label="Description" value={text(command.description)} />
        <Field label="Note" value={text(command.note)} />
        <Field label="Outcome" value={text(command.outcome)} />
      </dl>
      <Items label="Principles" value={command.principles} />
      <Items label="Assumptions" value={command.assumptions} />
      {phases.length > 0 && (
        <ol className="tr-plan__phases" aria-label="Planned phases">
          {phases.map((phase, index) => (
            <li key={index}>
              <strong>{text(phase.title)}</strong>
              {text(phase.size) && <span className="tr-muted"> · {text(phase.size)}</span>}
              {text(phase.description) && <div>{text(phase.description)}</div>}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}

function Field({ label, value }: { label: string; value?: string }) {
  if (!value) return null;
  return <><dt>{label}</dt><dd>{value}</dd></>;
}

function Items({ label, value }: { label: string; value: unknown }) {
  if (!Array.isArray(value)) return null;
  const items = value.filter((item): item is string => typeof item === "string");
  if (items.length === 0) return null;
  return <div className="tr-plan__items"><span>{label}</span><ul>{items.map((item, i) => <li key={i}>{item}</li>)}</ul></div>;
}
