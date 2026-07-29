use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ignore::WalkBuilder;
use tree_sitter::{Language as TsLanguage, Parser, Point, Tree};

const DEFAULT_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const BINARY_PROBE_BYTES: usize = 8192;
const IGNORED_DIR_NAMES: &[&str] = &[
    ".git",
    ".cache",
    ".next",
    ".nuxt",
    ".parcel-cache",
    ".pytest_cache",
    ".ruff_cache",
    ".svelte-kit",
    ".tox",
    ".turbo",
    ".venv",
    "build",
    "coverage",
    "dist",
    "node_modules",
    "out",
    "storybook-static",
    "target",
    "venv",
    "__pycache__",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Json,
    Yaml,
    Toml,
    Markdown,
}

impl SourceLanguage {
    pub const SUPPORTED: [Self; 8] = [
        Self::Rust,
        Self::TypeScript,
        Self::Tsx,
        Self::JavaScript,
        Self::Json,
        Self::Yaml,
        Self::Toml,
        Self::Markdown,
    ];

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "js" | "mjs" | "cjs" | "jsx" => Some(Self::JavaScript),
            "json" | "jsonc" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Markdown => "markdown",
        }
    }

    fn tree_sitter_language(self) -> TsLanguage {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    Parsed,
    Partial,
    Failed,
}

impl ParseStatus {
    pub fn as_db_str(self) -> &'static str {
        match self {
            Self::Parsed => "parsed",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRange {
    pub start: SourcePosition,
    pub end: SourcePosition,
}

impl SourceRange {
    pub fn from_node(node: tree_sitter::Node<'_>) -> Self {
        Self {
            start: LineIndex::point_to_position(node.start_position()),
            end: LineIndex::point_to_position(node.end_position()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    line_starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        for (idx, byte) in source.bytes().enumerate() {
            if byte == b'\n' && idx + 1 < source.len() {
                line_starts.push(idx + 1);
            }
        }
        Self {
            line_starts,
            len: source.len(),
        }
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn byte_to_position(&self, byte_offset: usize) -> anyhow::Result<SourcePosition> {
        if byte_offset > self.len {
            anyhow::bail!(
                "byte offset {byte_offset} exceeds source length {}",
                self.len
            );
        }
        let line_idx = match self.line_starts.binary_search(&byte_offset) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        };
        Ok(SourcePosition {
            line: line_idx + 1,
            column: byte_offset - self.line_starts[line_idx] + 1,
        })
    }

    pub fn point_to_position(point: Point) -> SourcePosition {
        SourcePosition {
            line: point.row + 1,
            column: point.column + 1,
        }
    }
}

pub struct ParsedSource {
    pub language: SourceLanguage,
    pub status: ParseStatus,
    pub error_count: usize,
    pub line_index: LineIndex,
    pub tree: Tree,
}

#[derive(Default)]
pub struct SourceParser {
    parser: Parser,
}

impl SourceParser {
    pub fn parse(
        &mut self,
        language: SourceLanguage,
        source: &str,
    ) -> anyhow::Result<ParsedSource> {
        self.parser
            .set_language(&language.tree_sitter_language())
            .with_context(|| format!("load {} parser", language.as_str()))?;
        let Some(tree) = self.parser.parse(source, None) else {
            anyhow::bail!("tree-sitter returned no tree for {}", language.as_str());
        };
        let error_count = count_error_nodes(&tree);
        let status = if tree.root_node().has_error() {
            ParseStatus::Partial
        } else {
            ParseStatus::Parsed
        };
        Ok(ParsedSource {
            language,
            status,
            error_count,
            line_index: LineIndex::new(source),
            tree,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileCandidate {
    pub path: PathBuf,
    pub language: SourceLanguage,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct SourceWalkOptions {
    pub max_file_bytes: u64,
}

impl Default for SourceWalkOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

pub fn discover_source_files(
    root: &Path,
    options: &SourceWalkOptions,
) -> anyhow::Result<Vec<SourceFileCandidate>> {
    let mut files = Vec::new();
    let mut builder = WalkBuilder::new(root);
    builder
        .require_git(false)
        .filter_entry(|entry| should_descend(entry.path()));
    let walker = builder.build();
    for entry in walker {
        let entry = entry.with_context(|| format!("walk {}", root.display()))?;
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        if let Some(candidate) = source_file_candidate(entry.path(), options)? {
            files.push(candidate);
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub fn source_file_candidate(
    path: &Path,
    options: &SourceWalkOptions,
) -> anyhow::Result<Option<SourceFileCandidate>> {
    let Some(language) = SourceLanguage::from_path(path) else {
        return Ok(None);
    };
    let metadata = fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    if !metadata.is_file() {
        return Ok(None);
    }
    let size_bytes = metadata.len();
    if size_bytes > options.max_file_bytes || looks_binary(path)? {
        return Ok(None);
    }
    Ok(Some(SourceFileCandidate {
        path: path.to_path_buf(),
        language,
        size_bytes,
    }))
}

pub fn source_file_candidate_in_root(
    root: &Path,
    path: &Path,
    options: &SourceWalkOptions,
) -> anyhow::Result<Option<SourceFileCandidate>> {
    if is_ignored_path_in_root(root, path)? {
        return Ok(None);
    }
    source_file_candidate(path, options)
}

fn should_descend(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    !is_ignored_dir_name(name)
}

pub(super) fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIR_NAMES.contains(&name)
}

pub(super) fn is_ignored_path_in_root(root: &Path, path: &Path) -> anyhow::Result<bool> {
    let relative = path
        .strip_prefix(root)
        .with_context(|| format!("{} is outside {}", path.display(), root.display()))?;
    Ok(relative.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(is_ignored_dir_name)
    }))
}

fn looks_binary(path: &Path) -> anyhow::Result<bool> {
    let bytes = fs::read(path)
        .with_context(|| format!("read {}", path.display()))?
        .into_iter()
        .take(BINARY_PROBE_BYTES)
        .collect::<Vec<_>>();
    Ok(bytes.contains(&0))
}

fn count_error_nodes(tree: &Tree) -> usize {
    let mut count = 0;
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.is_error() || node.is_missing() {
            count += 1;
        }
        for child_idx in 0..node.child_count() {
            if let Some(child) = node.child(child_idx) {
                stack.push(child);
            }
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_index_maps_bytes_to_one_based_positions() {
        let index = LineIndex::new("α\nlet x = 1;\n");

        assert_eq!(
            index.byte_to_position(0).unwrap(),
            SourcePosition { line: 1, column: 1 }
        );
        assert_eq!(
            index.byte_to_position("α\n".len()).unwrap(),
            SourcePosition { line: 2, column: 1 }
        );
        assert_eq!(index.line_count(), 2);
    }

    #[test]
    fn parser_reports_partial_results_for_syntax_errors() {
        let mut parser = SourceParser::default();
        let parsed = parser
            .parse(SourceLanguage::Rust, "fn broken( {\nlet x = 1;\n")
            .unwrap();

        assert_eq!(parsed.language, SourceLanguage::Rust);
        assert_eq!(parsed.status, ParseStatus::Partial);
        assert!(parsed.error_count > 0);
        assert_eq!(parsed.line_index.line_count(), 2);
    }

    #[test]
    fn parser_loads_initial_language_set() {
        let examples = [
            (SourceLanguage::Rust, "fn main() {}\n"),
            (SourceLanguage::TypeScript, "const x: number = 1;\n"),
            (SourceLanguage::Tsx, "const x = <div />;\n"),
            (SourceLanguage::JavaScript, "const x = 1;\n"),
            (SourceLanguage::Json, "{\"x\":1}\n"),
            (SourceLanguage::Yaml, "x: 1\n"),
            (SourceLanguage::Toml, "x = 1\n"),
            (SourceLanguage::Markdown, "# Title\n\nText\n"),
        ];
        let mut parser = SourceParser::default();
        for (language, source) in examples {
            let parsed = parser.parse(language, source).unwrap();
            assert_eq!(parsed.status, ParseStatus::Parsed, "{language:?}");
        }
    }

    #[test]
    fn walker_respects_language_size_binary_and_generated_filters() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(root.join("blob.rs"), b"fn main() {}\0").unwrap();
        fs::write(root.join("large.rs"), "012345678901234567890").unwrap();
        fs::write(root.join("notes.txt"), "ignore me\n").unwrap();
        fs::write(root.join(".gitignore"), "ignored-output/\n").unwrap();
        fs::create_dir(root.join("ignored-output")).unwrap();
        fs::write(
            root.join("ignored-output").join("generated.rs"),
            "fn generated() {}\n",
        )
        .unwrap();
        fs::create_dir(root.join("build")).unwrap();
        fs::write(
            root.join("build").join("generated.rs"),
            "fn generated() {}\n",
        )
        .unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(
            root.join("target").join("generated.rs"),
            "fn generated() {}\n",
        )
        .unwrap();

        let files = discover_source_files(root, &SourceWalkOptions { max_file_bytes: 20 }).unwrap();

        assert_eq!(
            files,
            vec![SourceFileCandidate {
                path: root.join("main.rs"),
                language: SourceLanguage::Rust,
                size_bytes: 13,
            }]
        );
    }

    #[test]
    fn direct_candidate_skips_common_generated_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("build")).unwrap();
        let path = root.join("build").join("generated.rs");
        fs::write(&path, "fn generated() {}\n").unwrap();

        let candidate =
            source_file_candidate_in_root(root, &path, &SourceWalkOptions::default()).unwrap();

        assert_eq!(candidate, None);
    }
}
