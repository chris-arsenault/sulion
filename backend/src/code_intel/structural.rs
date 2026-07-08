use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use ast_grep_core::matcher::Pattern;
use ast_grep_core::meta_var::MetaVariable;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc, TSLanguage};
use ast_grep_core::{Language, PatternError};
use ast_grep_language::SupportLang;
use serde::Serialize;
use similar::TextDiff;

use super::parser::{
    discover_source_files, is_ignored_path_in_root, source_file_candidate_in_root, LineIndex,
    SourceLanguage, SourceWalkOptions,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructuralLanguage {
    Support(SupportLang),
    Toml,
    Markdown,
}

impl StructuralLanguage {
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        let lang = match normalized.as_str() {
            "rs" | "rust" => Self::Support(SupportLang::Rust),
            "ts" | "typescript" => Self::Support(SupportLang::TypeScript),
            "tsx" => Self::Support(SupportLang::Tsx),
            "js" | "jsx" | "javascript" => Self::Support(SupportLang::JavaScript),
            "json" => Self::Support(SupportLang::Json),
            "yaml" | "yml" => Self::Support(SupportLang::Yaml),
            "toml" => Self::Toml,
            "md" | "markdown" => Self::Markdown,
            _ => anyhow::bail!("unsupported structural language: {value}"),
        };
        Ok(lang)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Support(SupportLang::Rust) => "rust",
            Self::Support(SupportLang::TypeScript) => "typescript",
            Self::Support(SupportLang::Tsx) => "tsx",
            Self::Support(SupportLang::JavaScript) => "javascript",
            Self::Support(SupportLang::Json) => "json",
            Self::Support(SupportLang::Yaml) => "yaml",
            Self::Toml => "toml",
            Self::Markdown => "markdown",
            Self::Support(_) => "unsupported",
        }
    }

    fn matches_source_language(self, language: SourceLanguage) -> bool {
        matches!(
            (self, language),
            (Self::Support(SupportLang::Rust), SourceLanguage::Rust)
                | (
                    Self::Support(SupportLang::TypeScript),
                    SourceLanguage::TypeScript
                )
                | (Self::Support(SupportLang::Tsx), SourceLanguage::Tsx)
                | (
                    Self::Support(SupportLang::JavaScript),
                    SourceLanguage::JavaScript
                )
                | (Self::Support(SupportLang::Json), SourceLanguage::Json)
                | (Self::Support(SupportLang::Yaml), SourceLanguage::Yaml)
                | (Self::Toml, SourceLanguage::Toml)
                | (Self::Markdown, SourceLanguage::Markdown)
        )
    }
}

impl Language for StructuralLanguage {
    fn pre_process_pattern<'q>(&self, query: &'q str) -> std::borrow::Cow<'q, str> {
        match self {
            Self::Support(lang) => lang.pre_process_pattern(query),
            Self::Toml | Self::Markdown => preprocess_expando_pattern('µ', query),
        }
    }

    fn expando_char(&self) -> char {
        match self {
            Self::Support(lang) => lang.expando_char(),
            Self::Toml | Self::Markdown => 'µ',
        }
    }

    fn kind_to_id(&self, kind: &str) -> u16 {
        match self {
            Self::Support(lang) => lang.kind_to_id(kind),
            Self::Toml | Self::Markdown => self.get_ts_language().id_for_node_kind(kind, true),
        }
    }

    fn field_to_id(&self, field: &str) -> Option<u16> {
        match self {
            Self::Support(lang) => lang.field_to_id(field),
            Self::Toml | Self::Markdown => self
                .get_ts_language()
                .field_id_for_name(field)
                .map(|field| field.get()),
        }
    }

    fn build_pattern(
        &self,
        builder: &ast_grep_core::matcher::PatternBuilder,
    ) -> Result<Pattern, PatternError> {
        match self {
            Self::Support(lang) => lang.build_pattern(builder),
            Self::Toml | Self::Markdown => builder.build(|src| StrDoc::try_new(src, *self)),
        }
    }
}

impl LanguageExt for StructuralLanguage {
    fn get_ts_language(&self) -> TSLanguage {
        match self {
            Self::Support(lang) => lang.get_ts_language(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StructuralFile {
    pub absolute_path: PathBuf,
    pub relative_path: String,
}

#[derive(Debug, Default)]
pub struct StructuralSearchOutput {
    pub results: Vec<StructuralMatchResult>,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default)]
pub struct StructuralPatchOutput {
    pub matches: usize,
    pub files: Vec<StructuralPatchFileSummary>,
    pub diff: String,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuralMatchResult {
    pub path: String,
    pub kind: String,
    pub range: StructuralRange,
    pub text: String,
    pub context: String,
    pub captures: Vec<StructuralCaptureResult>,
    pub confidence: &'static str,
    pub freshness: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuralCaptureResult {
    pub name: String,
    pub index: usize,
    pub range: StructuralRange,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuralRange {
    pub path: String,
    pub start_line: i32,
    pub start_col: i32,
    pub end_line: i32,
    pub end_col: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuralPatchFileSummary {
    pub path: String,
    pub matches: usize,
}

pub fn discover_structural_files(
    root: &Path,
    target: &Path,
    language: StructuralLanguage,
    walk: &SourceWalkOptions,
) -> anyhow::Result<Vec<StructuralFile>> {
    if is_ignored_path_in_root(root, target)? {
        return Ok(Vec::new());
    }
    let candidates = if target.is_file() {
        source_file_candidate_in_root(root, target, walk)?
            .into_iter()
            .collect()
    } else {
        discover_source_files(target, walk)?
    };
    candidates
        .into_iter()
        .filter(|candidate| language.matches_source_language(candidate.language))
        .map(|candidate| {
            let relative_path = candidate
                .path
                .strip_prefix(root)
                .with_context(|| {
                    format!(
                        "{} is outside structural root {}",
                        candidate.path.display(),
                        root.display()
                    )
                })?
                .to_string_lossy()
                .replace('\\', "/");
            Ok(StructuralFile {
                absolute_path: candidate.path,
                relative_path,
            })
        })
        .collect()
}

pub fn search_files(
    files: &[StructuralFile],
    language: StructuralLanguage,
    pattern: &str,
    limit: usize,
) -> StructuralSearchOutput {
    let mut output = StructuralSearchOutput::default();
    for file in files {
        if output.results.len() > limit {
            break;
        }
        let source = match fs::read_to_string(&file.absolute_path) {
            Ok(source) => source,
            Err(err) => {
                output
                    .warnings
                    .push(format!("{}: {err}", file.relative_path));
                continue;
            }
        };
        let pattern = match Pattern::try_new(pattern, language) {
            Ok(pattern) => pattern,
            Err(err) => {
                output
                    .warnings
                    .push(format!("invalid ast-grep pattern: {err}"));
                break;
            }
        };
        let root = match ast_grep_core::tree_sitter::StrDoc::try_new(&source, language) {
            Ok(doc) => ast_grep_core::AstGrep::doc(doc),
            Err(err) => {
                output
                    .warnings
                    .push(format!("{}: parse failed: {err}", file.relative_path));
                continue;
            }
        };
        let line_index = LineIndex::new(&source);
        let root_node = root.root();
        for matched in root_node.find_all(&pattern) {
            if output.results.len() >= limit {
                output.truncated = true;
                break;
            }
            output.results.push(match_result(
                &file.relative_path,
                &source,
                &line_index,
                &matched,
            ));
        }
    }
    output
}

pub fn patch_files(
    files: &[StructuralFile],
    language: StructuralLanguage,
    pattern: &str,
    rewrite: &str,
    limit: usize,
) -> StructuralPatchOutput {
    let mut output = StructuralPatchOutput::default();
    let mut remaining = limit;
    for file in files {
        if remaining == 0 {
            output.truncated = true;
            break;
        }
        let source = match fs::read_to_string(&file.absolute_path) {
            Ok(source) => source,
            Err(err) => {
                output
                    .warnings
                    .push(format!("{}: {err}", file.relative_path));
                continue;
            }
        };
        let pattern = match Pattern::try_new(pattern, language) {
            Ok(pattern) => pattern,
            Err(err) => {
                output
                    .warnings
                    .push(format!("invalid ast-grep pattern: {err}"));
                break;
            }
        };
        let root = match ast_grep_core::tree_sitter::StrDoc::try_new(&source, language) {
            Ok(doc) => ast_grep_core::AstGrep::doc(doc),
            Err(err) => {
                output
                    .warnings
                    .push(format!("{}: parse failed: {err}", file.relative_path));
                continue;
            }
        };
        let mut edits = root.root().replace_all(&pattern, rewrite);
        if edits.len() > remaining {
            edits.truncate(remaining);
            output.truncated = true;
        }
        if edits.is_empty() {
            continue;
        }
        remaining = remaining.saturating_sub(edits.len());
        output.matches += edits.len();
        let patched = apply_edits(&source, &edits);
        if patched != source {
            output
                .diff
                .push_str(&unified_diff(&file.relative_path, &source, &patched));
        }
        output.files.push(StructuralPatchFileSummary {
            path: file.relative_path.clone(),
            matches: edits.len(),
        });
    }
    output
}

fn match_result(
    relative_path: &str,
    source: &str,
    line_index: &LineIndex,
    matched: &ast_grep_core::NodeMatch<'_, ast_grep_core::tree_sitter::StrDoc<StructuralLanguage>>,
) -> StructuralMatchResult {
    let range = matched.range();
    StructuralMatchResult {
        path: relative_path.to_string(),
        kind: matched.kind().to_string(),
        range: structural_range(relative_path, line_index, range.clone()),
        text: truncate_text(matched.text().as_ref(), 240),
        context: context_excerpt(source, &range, 1),
        captures: captures(relative_path, line_index, matched),
        confidence: "syntactic",
        freshness: "fresh",
    }
}

fn captures(
    relative_path: &str,
    line_index: &LineIndex,
    matched: &ast_grep_core::NodeMatch<'_, ast_grep_core::tree_sitter::StrDoc<StructuralLanguage>>,
) -> Vec<StructuralCaptureResult> {
    let mut captures = Vec::new();
    for meta_var in matched.get_env().get_matched_variables() {
        match meta_var {
            MetaVariable::Capture(name, _) => {
                if let Some(node) = matched.get_env().get_match(&name) {
                    captures.push(StructuralCaptureResult {
                        name,
                        index: 0,
                        range: structural_range(relative_path, line_index, node.range()),
                        text: truncate_text(node.text().as_ref(), 160),
                    });
                }
            }
            MetaVariable::MultiCapture(name) => {
                for (index, node) in matched
                    .get_env()
                    .get_multiple_matches(&name)
                    .into_iter()
                    .enumerate()
                {
                    captures.push(StructuralCaptureResult {
                        name: name.clone(),
                        index,
                        range: structural_range(relative_path, line_index, node.range()),
                        text: truncate_text(node.text().as_ref(), 160),
                    });
                }
            }
            MetaVariable::Dropped(_) | MetaVariable::Multiple => {}
        }
    }
    captures.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.index.cmp(&right.index))
            .then(left.range.start_line.cmp(&right.range.start_line))
            .then(left.range.start_col.cmp(&right.range.start_col))
    });
    captures
}

fn structural_range(
    relative_path: &str,
    line_index: &LineIndex,
    range: std::ops::Range<usize>,
) -> StructuralRange {
    let start = line_index
        .byte_to_position(range.start)
        .expect("ast-grep start byte should be in source");
    let end = line_index
        .byte_to_position(range.end)
        .expect("ast-grep end byte should be in source");
    StructuralRange {
        path: relative_path.to_string(),
        start_line: start.line as i32,
        start_col: start.column as i32,
        end_line: end.line as i32,
        end_col: end.column as i32,
    }
}

fn context_excerpt(source: &str, range: &std::ops::Range<usize>, context_lines: usize) -> String {
    let line_index = LineIndex::new(source);
    let start = line_index
        .byte_to_position(range.start)
        .map(|position| position.line)
        .unwrap_or(1);
    let end = line_index
        .byte_to_position(range.end)
        .map(|position| position.line)
        .unwrap_or(start);
    let first = start.saturating_sub(context_lines + 1) + 1;
    let last = end.saturating_add(context_lines);
    let mut out = String::new();
    for (idx, line) in source.lines().enumerate() {
        let line_number = idx + 1;
        if line_number < first || line_number > last {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    truncate_text(&out, 800)
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx == max_chars {
            out.push_str("...");
            return out;
        }
        out.push(ch);
    }
    out
}

fn apply_edits(source: &str, edits: &[ast_grep_core::source::Edit<String>]) -> String {
    let mut patched = source.to_string();
    for edit in edits.iter().rev() {
        let replacement = String::from_utf8_lossy(&edit.inserted_text);
        patched.replace_range(
            edit.position..edit.position + edit.deleted_length,
            replacement.as_ref(),
        );
    }
    patched
}

fn unified_diff(path: &str, old: &str, new: &str) -> String {
    let old_path = format!("a/{path}");
    let new_path = format!("b/{path}");
    let mut diff = TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&old_path, &new_path)
        .to_string();
    if !diff.ends_with('\n') {
        diff.push('\n');
    }
    diff
}

fn preprocess_expando_pattern(expando: char, query: &str) -> std::borrow::Cow<'_, str> {
    let mut ret = Vec::with_capacity(query.len());
    let mut dollar_count = 0;
    for ch in query.chars() {
        if ch == '$' {
            dollar_count += 1;
            continue;
        }
        let need_replace = matches!(ch, 'A'..='Z' | '_') || dollar_count == 3;
        let sigil = if need_replace { expando } else { '$' };
        ret.extend(std::iter::repeat_n(sigil, dollar_count));
        dollar_count = 0;
        ret.push(ch);
    }
    let sigil = if dollar_count == 3 { expando } else { '$' };
    ret.extend(std::iter::repeat_n(sigil, dollar_count));
    std::borrow::Cow::Owned(ret.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structural_files(root: &Path, language: StructuralLanguage) -> Vec<StructuralFile> {
        discover_structural_files(root, root, language, &SourceWalkOptions::default()).unwrap()
    }

    #[test]
    fn structural_search_returns_ranges_captures_and_context() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(
            root.join("lib.rs"),
            "fn one() -> i32 {\n    foo(1)\n}\nfn two() -> i32 {\n    foo(2)\n}\n",
        )
        .unwrap();
        let language = StructuralLanguage::parse("rust").unwrap();
        let output = search_files(&structural_files(root, language), language, "foo($A)", 10);

        assert!(!output.truncated);
        assert_eq!(output.results.len(), 2);
        assert_eq!(output.results[0].range.path, "lib.rs");
        assert_eq!(output.results[0].range.start_line, 2);
        assert_eq!(output.results[0].captures[0].name, "A");
        assert_eq!(output.results[0].captures[0].text, "1");
        assert!(output.results[0].context.contains("foo(1)"));
    }

    #[test]
    fn structural_search_reports_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("lib.rs"), "fn main() { foo(1); foo(2); }\n").unwrap();
        let language = StructuralLanguage::parse("rust").unwrap();
        let output = search_files(&structural_files(root, language), language, "foo($A)", 1);

        assert!(output.truncated);
        assert_eq!(output.results.len(), 1);
    }

    #[test]
    fn structural_search_respects_gitignore_and_generated_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join(".gitignore"), "ignored-output/\n").unwrap();
        fs::write(root.join("lib.rs"), "fn main() { foo(1); }\n").unwrap();
        fs::create_dir(root.join("ignored-output")).unwrap();
        fs::write(
            root.join("ignored-output").join("generated.rs"),
            "fn generated() { foo(2); }\n",
        )
        .unwrap();
        fs::create_dir(root.join("build")).unwrap();
        fs::write(
            root.join("build").join("generated.rs"),
            "fn generated() { foo(3); }\n",
        )
        .unwrap();
        let language = StructuralLanguage::parse("rust").unwrap();

        let output = search_files(&structural_files(root, language), language, "foo($A)", 10);

        assert_eq!(output.results.len(), 1);
        assert_eq!(output.results[0].range.path, "lib.rs");
        assert_eq!(output.results[0].captures[0].text, "1");
    }

    #[test]
    fn structural_patch_returns_diff_without_writing_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let path = root.join("lib.rs");
        fs::write(&path, "fn main() {\n    foo(1);\n}\n").unwrap();
        let language = StructuralLanguage::parse("rust").unwrap();
        let output = patch_files(
            &structural_files(root, language),
            language,
            "foo($A)",
            "bar($A)",
            10,
        );

        assert_eq!(output.matches, 1);
        assert!(!output.truncated);
        assert!(output.diff.contains("--- a/lib.rs"));
        assert!(output.diff.contains("+++ b/lib.rs"));
        assert!(output.diff.contains("-    foo(1);"));
        assert!(output.diff.contains("+    bar(1);"));
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "fn main() {\n    foo(1);\n}\n"
        );
    }

    #[test]
    fn structural_language_supports_toml_and_markdown() {
        assert_eq!(StructuralLanguage::parse("toml").unwrap().as_str(), "toml");
        assert_eq!(
            StructuralLanguage::parse("markdown").unwrap().as_str(),
            "markdown"
        );
    }
}
