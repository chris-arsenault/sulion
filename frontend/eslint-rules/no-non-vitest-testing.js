const FORBIDDEN = new Set([
  "jest",
  "@jest/globals",
  "mocha",
  "chai",
  "jasmine",
  "ava",
  "tape",
]);

export default {
  meta: {
    type: "problem",
    docs: { description: "Vitest is the only allowed test framework." },
    schema: [],
    messages: { forbidden: "Use Vitest only. `{{source}}` is not allowed." },
  },
  create(context) {
    return {
      ImportDeclaration(node) {
        const src = node.source.value;
        if (typeof src === "string" && FORBIDDEN.has(src)) {
          context.report({ node, messageId: "forbidden", data: { source: src } });
        }
      },
    };
  },
};
