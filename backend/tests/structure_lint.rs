use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{File, ImplItemFn, ItemFn, ItemImpl};
use walkdir::WalkDir;

const DEFAULT_MAX_FILE_LINES: usize = 900;
const DEFAULT_MAX_FUNCTION_LINES: usize = 140;
const DEFAULT_MAX_IMPL_LINES: usize = 400;

const FILE_LIMIT_OVERRIDES: &[(&str, usize)] = &[("src/ingest/ingester.rs", 1200)];

#[test]
fn rust_source_stays_within_structural_limits() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");
    let mut failures = Vec::new();

    for entry in WalkDir::new(&src_dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("rs")
        {
            continue;
        }

        let rel = path
            .strip_prefix(&manifest_dir)
            .expect("source file lives under manifest dir");
        let source = fs::read_to_string(path).expect("read source file");
        let file_lines = source.lines().count();
        let file_limit = file_limit_for(rel);
        if file_lines > file_limit {
            failures.push(format!(
                "{} has {} lines (limit {})",
                rel.display(),
                file_lines,
                file_limit
            ));
        }

        let syntax = syn::parse_file(&source).unwrap_or_else(|error| {
            panic!(
                "failed to parse {} for structure lint: {error}",
                rel.display()
            )
        });
        let mut visitor = StructureVisitor::new(rel);
        visitor.visit_file(&syntax);
        failures.extend(visitor.failures);
    }

    assert!(
        failures.is_empty(),
        "rust structure lint failures:\n{}",
        failures.join("\n")
    );
}

/// `api` is the HTTP layer: it may depend on the domain, never the reverse.
///
/// The node runtime used to call into `api::repo_lifecycle_routes` and
/// `api::file_content`, which meant the process that owns the repos directory
/// depended on the process that serves requests about it, and two modules were
/// made `pub(crate)` purely to allow it. Domain logic that both need lives in
/// its own module now; this keeps it there.
#[test]
fn only_the_api_layer_depends_on_the_api_layer() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");
    let mut failures = Vec::new();

    for entry in WalkDir::new(&src_dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("rs")
        {
            continue;
        }
        let rel = path
            .strip_prefix(&manifest_dir)
            .expect("source file lives under manifest dir");
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        // `api` itself, and the code-intel service's own unrelated `api` module.
        if rel_str.starts_with("src/api/") || rel_str.starts_with("src/code_intel/") {
            continue;
        }
        let source = fs::read_to_string(path).expect("read source file");
        for (index, line) in source.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("crate::api::") || code.contains("crate::api;") {
                failures.push(format!("{}:{}: {}", rel.display(), index + 1, line.trim()));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "the api layer must not be a dependency of the domain:\n{}",
        failures.join("\n")
    );
}

fn file_limit_for(path: &Path) -> usize {
    let path = path.to_string_lossy();
    FILE_LIMIT_OVERRIDES
        .iter()
        .find_map(|(candidate, limit)| (*candidate == path).then_some(*limit))
        .unwrap_or(DEFAULT_MAX_FILE_LINES)
}

fn span_lines(span: Span) -> usize {
    let start = span.start().line;
    let end = span.end().line;
    end.saturating_sub(start) + 1
}

struct StructureVisitor<'a> {
    path: &'a Path,
    failures: Vec<String>,
}

impl<'a> StructureVisitor<'a> {
    fn new(path: &'a Path) -> Self {
        Self {
            path,
            failures: Vec::new(),
        }
    }

    fn check_limit(&mut self, label: &str, span: Span, limit: usize) {
        let lines = span_lines(span);
        if lines > limit {
            self.failures.push(format!(
                "{}: {} spans {} lines (limit {})",
                self.path.display(),
                label,
                lines,
                limit
            ));
        }
    }
}

impl<'ast> Visit<'ast> for StructureVisitor<'_> {
    fn visit_file(&mut self, node: &'ast File) {
        syn::visit::visit_file(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.check_limit(
            &format!("fn {}", node.sig.ident),
            node.span(),
            DEFAULT_MAX_FUNCTION_LINES,
        );
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.check_limit(
            &format!("fn {}", node.sig.ident),
            node.span(),
            DEFAULT_MAX_FUNCTION_LINES,
        );
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        self.check_limit("impl block", node.span(), DEFAULT_MAX_IMPL_LINES);
        syn::visit::visit_item_impl(self, node);
    }
}
