export default {
  meta: {
    type: "suggestion",
    docs: {
      description:
        "Discourage raw `T | undefined` unions in type annotations; prefer a named alias or `?:` shorthand.",
    },
    schema: [],
    messages: {
      raw: "Avoid raw `T | undefined` here. Use a named type alias (e.g., `type Maybe<T>`) or `prop?: T`.",
    },
  },
  create(context) {
    return {
      TSUnionType(node) {
        const hasUndefined = node.types.some(
          (t) => t.type === "TSUndefinedKeyword"
        );
        if (!hasUndefined) return;
        const parent = node.parent;
        if (parent?.type === "TSTypeAliasDeclaration") return;
        if (parent?.type === "TSPropertySignature" && parent.optional) return;
        context.report({ node, messageId: "raw" });
      },
    };
  },
};
