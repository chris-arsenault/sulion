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
