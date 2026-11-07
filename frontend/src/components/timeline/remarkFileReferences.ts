import type {
  Definition,
  Image,
  Link,
  Parent,
  PhrasingContent,
  Root,
} from "mdast";

import { classifyMarkdownLink, type FileLinkTarget } from "./markdownLinks";

const IMAGE_PATH = /\.(?:apng|avif|bmp|gif|ico|jpe?g|png|svg|webp)$/i;
const FILE_EXTENSION = /\.([a-z][a-z0-9]*)(?::\d+(?::\d+)?)?(?:[#?].*)?$/i;
const COMMON_EXTENSIONS = new Set([
  "apng",
  "avif",
  "bmp",
  "gif",
  "ico",
  "jpeg",
  "jpg",
  "png",
  "svg",
  "webp",
  "md",
  "mdx",
  "txt",
  "pdf",
  "csv",
  "tsv",
  "json",
  "jsonl",
  "ndjson",
  "yaml",
  "yml",
  "toml",
  "rs",
  "ts",
  "tsx",
  "js",
  "jsx",
  "py",
  "rb",
  "go",
  "sh",
  "css",
  "html",
  "sql",
  "xml",
  "log",
]);
const TOKENS = /[^\s<>"'`()]+/g;

interface Context {
  target: FileLinkTarget;
  directory: string;
  definitions: Map<string, Definition>;
}

/** Resolve references in parsed prose, including an announced directory followed
 * by a filename list. Fenced code, existing link labels, and raw HTML stay literal. */
export function remarkFileReferences(target: FileLinkTarget | null) {
  return (tree: Root) => {
    if (target)
      transformChildren(tree, {
        target,
        directory: "",
        definitions: new Map(
          tree.children
            .filter((node): node is Definition => node.type === "definition")
            .map((node) => [node.identifier.toLowerCase(), node]),
        ),
      });
  };
}

function transformChildren(parent: Parent, context: Context) {
  const next: Parent["children"] = [];
  for (const node of parent.children) {
    if (node.type === "heading") context.directory = "";
    if (node.type === "code" || node.type === "html") {
      next.push(node);
    } else if (node.type === "link") {
      next.push(resolveLink(node, context));
    } else if (node.type === "linkReference") {
      const definition = context.definitions.get(node.identifier.toLowerCase());
      next.push(
        definition
          ? resolveLink(
              {
                type: "link",
                url: definition.url,
                title: definition.title,
                children: node.children,
              },
              context,
            )
          : node,
      );
    } else if (node.type === "imageReference") {
      const definition = context.definitions.get(node.identifier.toLowerCase());
      next.push(
        definition
          ? resolveImage(
              {
                type: "image",
                url: definition.url,
                alt: node.alt,
                title: definition.title,
              },
              context,
            )
          : node,
      );
    } else if (node.type === "image") {
      next.push(resolveImage(node, context));
    } else if (node.type === "inlineCode") {
      next.push(reference(node.value, node, context) ?? node);
    } else if (node.type === "text") {
      next.push(...transformText(node.value, context));
    } else {
      if ("children" in node) {
        // Directories declared outside a list apply to its items. A declaration
        // inside a quote/list item stays in that local block.
        const nested =
          node.type === "listItem" || node.type === "blockquote"
            ? { ...context }
            : context;
        transformChildren(node, nested);
      }
      next.push(node);
    }
  }
  parent.children = next;
}

function resolveImage(node: Image, context: Context): Image | Link {
  const file = resolveReference(node.url, context);
  if (file.kind !== "file") return node;
  node.url = fileHref(file.path);
  return { type: "link", url: node.url, children: [node] };
}

function transformText(value: string, context: Context): PhrasingContent[] {
  const result: PhrasingContent[] = [];
  let last = 0;
  for (const token of value.matchAll(TOKENS)) {
    const candidate = token[0]
      .replace(/^[[{]+/, "")
      .replace(/[\]},;.!?:]+$/, "");
    const start = token.index! + token[0].indexOf(candidate);
    const linked = reference(
      candidate,
      { type: "text", value: candidate },
      context,
    );
    if (!linked) continue;
    if (start > last)
      result.push({ type: "text", value: value.slice(last, start) });
    result.push(linked);
    last = start + candidate.length;
  }
  if (last < value.length)
    result.push({ type: "text", value: value.slice(last) });
  return result;
}

function reference(
  value: string,
  label: PhrasingContent,
  context: Context,
): Link | null {
  if (value.endsWith("/")) {
    const directory = classifyMarkdownLink(
      value,
      context.target.repo,
      context.target.rootPath,
    );
    if (directory.kind === "file") context.directory = `${directory.path}/`;
    return null;
  }
  const extension = FILE_EXTENSION.exec(value)?.[1]?.toLowerCase();
  if (!extension || (!value.includes("/") && !COMMON_EXTENSIONS.has(extension)))
    return null;
  const file = resolveReference(value, context);
  if (file.kind !== "file") return null;
  return resolveLink(
    { type: "link", url: fileHref(file.path, file.line), children: [label] },
    context,
  );
}

function resolveReference(value: string, { target, directory }: Context) {
  const relative =
    directory && !value.includes("/") && !value.includes("://")
      ? `${directory}${value}`
      : value;
  return classifyMarkdownLink(relative, target.repo, target.rootPath);
}

function resolveLink(link: Link, context: Context): Link {
  const file = resolveReference(link.url, context);
  if (file.kind !== "file") return link;
  link.url = fileHref(file.path, file.line);
  // Resolve image children against the same directory, without linking labels
  // again or nesting another anchor inside an existing Markdown link.
  const resolveImages = (parent: Parent) => {
    for (const node of parent.children) {
      if (node.type === "image") {
        const image = resolveReference(node.url, context);
        if (image.kind === "file") node.url = fileHref(image.path);
      } else if ("children" in node) resolveImages(node);
    }
  };
  resolveImages(link);
  if (IMAGE_PATH.test(file.path) && !hasImage(link)) {
    const image: Image = {
      type: "image",
      url: fileHref(file.path),
      alt: file.path.split("/").at(-1) ?? file.path,
    };
    link.children.push(image);
  }
  return link;
}

function hasImage(parent: Parent): boolean {
  return parent.children.some(
    (node) => node.type === "image" || ("children" in node && hasImage(node)),
  );
}

function fileHref(path: string, line?: number) {
  const suffix = line == null ? "" : `#L${line}`;
  return encodeURI(path) + suffix;
}
