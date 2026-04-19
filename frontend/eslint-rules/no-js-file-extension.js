import path from "node:path";

const ALLOWED_DIRS = ["eslint-rules", "docker"];
const ALLOWED_FILES = new Set([
  "eslint.config.js",
  "prettier.config.js",
  "vite.config.js",
  "vitest.config.js",
]);

export default {
  meta: {
    type: "problem",
    docs: { description: "Source files must be `.ts`/`.tsx`, not `.js`/`.jsx`." },
    schema: [],
    messages: {
      noJs: "Source files must be `.ts` or `.tsx`. Rename `{{file}}`.",
    },
  },
  create(context) {
    return {
      Program(node) {
        const filename = context.filename ?? context.getFilename();
        const ext = path.extname(filename);
        if (ext !== ".js" && ext !== ".jsx") return;
        const base = path.basename(filename);
        if (ALLOWED_FILES.has(base)) return;
        const parts = filename.split(path.sep);
        if (parts.some((p) => ALLOWED_DIRS.includes(p))) return;
        context.report({ node, messageId: "noJs", data: { file: base } });
      },
    };
  },
};
