import type { Maybe } from "../../../lib/types";

export type PlanCommand = Record<string, unknown>;

export function record(value: unknown): value is PlanCommand {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function planCommands(input: unknown): PlanCommand[] {
  if (!record(input) || !Array.isArray(input.plan_commands)) return [];
  return input.plan_commands.filter((value): value is PlanCommand =>
    record(value) && typeof value.action === "string",
  );
}

export function text(value: unknown): Maybe<string> {
  return typeof value === "string" ? value : undefined;
}

export function planCommandSummary(input: unknown): string {
  const commands = planCommands(input);
  const first = commands[0];
  if (!first) return "";
  const parts = [
    text(first.action),
    text(first.title),
    typeof first.phase === "string" ? `phase ${first.phase}` : undefined,
    text(first.status),
    typeof first.phase_count === "number" ? `${first.phase_count} phases` : undefined,
  ].filter(Boolean);
  if (commands.length > 1) parts.push(`+${commands.length - 1} commands`);
  return parts.join(" · ");
}
