const DEFAULT_MAX = 12;

export default {
  meta: {
    type: "suggestion",
    docs: {
      description: "Limit JSX prop count; refactor to a Parameter Object beyond the cap.",
    },
    schema: [{ type: "object", properties: { max: { type: "number" } } }],
    messages: {
      tooMany:
        "Component <{{name}}> has {{count}} JSX props (>{{max}}). Refactor to a single props object.",
    },
  },
  create(context) {
    const max = context.options[0]?.max ?? DEFAULT_MAX;
    return {
      JSXOpeningElement(node) {
        const count = node.attributes.filter((a) => a.type === "JSXAttribute").length;
        if (count > max) {
          const name =
            node.name.type === "JSXIdentifier" ? node.name.name : "anonymous";
          context.report({ node, messageId: "tooMany", data: { name, count, max } });
        }
      },
    };
  },
};
