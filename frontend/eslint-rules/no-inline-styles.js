export default {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow inline `style={{...}}` on JSX. Use a co-located CSS class instead.",
    },
    schema: [],
    messages: {
      inline:
        "Inline styles are forbidden. Move to a co-located .css file. For dynamic values only, add `// eslint-disable-next-line local/no-inline-styles` with a comment explaining why.",
    },
  },
  create(context) {
    return {
      JSXAttribute(node) {
        if (node.name?.name !== "style") return;
        if (node.value?.type !== "JSXExpressionContainer") return;
        context.report({ node, messageId: "inline" });
      },
    };
  },
};
