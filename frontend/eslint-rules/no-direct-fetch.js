import path from "node:path";

// Files that are allowed to call fetch directly. The idea is that the
// api/client.ts wrapper is the single front door for HTTP so auth,
// error shapes, and base URLs can be managed centrally. Web workers
// need their own fetch path — they don't have access to the module
// graph and the streaming parse work they do is also part of the
// low-level plumbing.
const ALLOWED_SUFFIXES = [
  path.join("src", "api", "client.ts"),
  ".worker.ts",
];

export default {
  meta: {
    type: "problem",
    docs: {
      description:
        "Disallow direct `fetch()` calls outside the api/client.ts wrapper and web workers.",
    },
    schema: [],
    messages: {
      direct:
        "Use functions from `api/client.ts` instead of calling `fetch` directly. The wrapper centralises base URLs, error shapes, and response parsing.",
    },
  },
  create(context) {
    const filename = context.filename ?? context.getFilename();
    if (ALLOWED_SUFFIXES.some((s) => filename.endsWith(s))) {
      return {};
    }
    return {
      CallExpression(node) {
        const c = node.callee;
        const isFetch =
          (c.type === "Identifier" && c.name === "fetch") ||
          (c.type === "MemberExpression" &&
            c.property?.type === "Identifier" &&
            c.property.name === "fetch");
        if (isFetch) context.report({ node, messageId: "direct" });
      },
    };
  },
};
