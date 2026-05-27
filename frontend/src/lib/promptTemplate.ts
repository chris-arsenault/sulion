export interface PromptTemplateVariable {
  name: string;
  occurrences: number;
}

export type PromptTemplateValues = Record<string, string>;

export function extractPromptTemplateVariables(
  template: string,
): PromptTemplateVariable[] {
  const orderedNames: string[] = [];
  const occurrences = new Map<string, number>();

  for (let index = 0; index < template.length;) {
    if (template[index] !== "$") {
      index += 1;
      continue;
    }

    const next = template[index + 1];
    if (next === "$") {
      index += 2;
      continue;
    }
    if (!isVariableStart(next)) {
      index += 1;
      continue;
    }

    let end = index + 2;
    while (isVariablePart(template[end])) end += 1;
    const name = template.slice(index + 1, end);
    if (!occurrences.has(name)) orderedNames.push(name);
    occurrences.set(name, (occurrences.get(name) ?? 0) + 1);
    index = end;
  }

  return orderedNames.map((name) => ({
    name,
    occurrences: occurrences.get(name) ?? 0,
  }));
}

export function renderPromptTemplate(
  template: string,
  values: PromptTemplateValues,
): string {
  let rendered = "";

  for (let index = 0; index < template.length;) {
    const char = template[index];
    if (char !== "$") {
      rendered += char;
      index += 1;
      continue;
    }

    const next = template[index + 1];
    if (next === "$") {
      rendered += "$";
      index += 2;
      continue;
    }
    if (!isVariableStart(next)) {
      rendered += "$";
      index += 1;
      continue;
    }

    let end = index + 2;
    while (isVariablePart(template[end])) end += 1;
    const name = template.slice(index + 1, end);
    rendered += values[name] ?? "";
    index = end;
  }

  return rendered;
}

function isVariableStart(char?: string): boolean {
  return char !== undefined && /[A-Za-z_]/.test(char);
}

function isVariablePart(char?: string): boolean {
  return char !== undefined && /[A-Za-z0-9_]/.test(char);
}
