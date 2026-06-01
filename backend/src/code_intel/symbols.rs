use std::collections::{HashMap, HashSet};
use std::path::Path;

use ring::digest;
use tree_sitter::Node;

use super::parser::{ParsedSource, SourceLanguage, SourceRange};

const MAX_SIGNATURE_BYTES: usize = 240;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedSymbol {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: String,
    pub name: String,
    pub qualified_name: String,
    pub signature: Option<String>,
    pub visibility: Option<String>,
    pub exported: Option<bool>,
    pub disambiguator: i32,
    pub name_range: SourceRange,
    pub decl_range: SourceRange,
    pub body_range: Option<SourceRange>,
    pub doc_range: Option<SourceRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedReference {
    pub referenced_name: String,
    pub reference_kind: String,
    pub range: SourceRange,
}

pub fn extract_symbols(
    parsed: &ParsedSource,
    source: &str,
    root_path: &Path,
    relative_path: &str,
) -> Vec<ExtractedSymbol> {
    let mut extractor = SymbolExtractor {
        parsed,
        source,
        root_path: &root_path.to_string_lossy(),
        relative_path,
        counts: HashMap::new(),
        out: Vec::new(),
    };
    extractor.visit(parsed.tree.root_node(), None, None);
    extractor.out
}

pub fn extract_references(
    parsed: &ParsedSource,
    source: &str,
    symbols: &[ExtractedSymbol],
) -> Vec<ExtractedReference> {
    let definition_names = symbols
        .iter()
        .map(|symbol| reference_key(&symbol.name, symbol.name_range))
        .collect::<HashSet<_>>();
    let mut extractor = ReferenceExtractor {
        parsed,
        source,
        definition_names,
        seen: HashSet::new(),
        out: Vec::new(),
    };
    extractor.visit(parsed.tree.root_node());
    extractor.out
}

struct SymbolExtractor<'a> {
    parsed: &'a ParsedSource,
    source: &'a str,
    root_path: &'a str,
    relative_path: &'a str,
    counts: HashMap<(String, String), i32>,
    out: Vec<ExtractedSymbol>,
}

impl SymbolExtractor<'_> {
    fn visit(
        &mut self,
        node: Node<'_>,
        parent_id: Option<String>,
        parent_qualified_name: Option<String>,
    ) {
        if let Some(seed) = self.symbol_seed(node, parent_qualified_name.as_deref()) {
            let count_key = (seed.kind.clone(), seed.qualified_name.clone());
            let disambiguator = self.counts.entry(count_key).or_insert(0);
            let id = symbol_id(
                self.root_path,
                self.relative_path,
                self.parsed.language,
                &seed.qualified_name,
                &seed.kind,
                *disambiguator,
            );
            let next_parent_id = Some(id.clone());
            let next_parent_qualified = Some(seed.qualified_name.clone());
            self.out.push(ExtractedSymbol {
                id,
                parent_id: parent_id.clone(),
                kind: seed.kind,
                name: seed.name,
                qualified_name: seed.qualified_name,
                signature: seed.signature,
                visibility: seed.visibility,
                exported: seed.exported,
                disambiguator: *disambiguator,
                name_range: seed.name_range,
                decl_range: seed.decl_range,
                body_range: seed.body_range,
                doc_range: seed.doc_range,
            });
            *disambiguator += 1;
            self.visit_children(node, next_parent_id, next_parent_qualified);
        } else {
            self.visit_children(node, parent_id, parent_qualified_name);
        }
    }

    fn visit_children(
        &mut self,
        node: Node<'_>,
        parent_id: Option<String>,
        parent_qualified_name: Option<String>,
    ) {
        for idx in 0..node.named_child_count() {
            if let Some(child) = node.named_child(idx) {
                self.visit(child, parent_id.clone(), parent_qualified_name.clone());
            }
        }
    }

    fn symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        match self.parsed.language {
            SourceLanguage::Rust => self.rust_symbol_seed(node, parent_qualified_name),
            SourceLanguage::TypeScript | SourceLanguage::Tsx | SourceLanguage::JavaScript => {
                self.javascript_family_symbol_seed(node, parent_qualified_name)
            }
            SourceLanguage::Json => self.json_symbol_seed(node, parent_qualified_name),
            SourceLanguage::Yaml => self.yaml_symbol_seed(node, parent_qualified_name),
            SourceLanguage::Toml => self.toml_symbol_seed(node, parent_qualified_name),
            SourceLanguage::Markdown => self.markdown_symbol_seed(node, parent_qualified_name),
        }
    }

    fn rust_symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        let (kind, name, name_node) = match node.kind() {
            "function_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("function", name, name_node)
            }
            "struct_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("struct", name, name_node)
            }
            "enum_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("enum", name, name_node)
            }
            "trait_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("trait", name, name_node)
            }
            "mod_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("module", name, name_node)
            }
            "const_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("constant", name, name_node)
            }
            "static_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("static", name, name_node)
            }
            "type_item" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("type", name, name_node)
            }
            "macro_definition" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("macro", name, name_node)
            }
            "impl_item" => {
                let name_node = node.child_by_field_name("type")?;
                ("impl", rust_impl_name(node, self.source)?, name_node)
            }
            _ => return None,
        };
        Some(self.seed_from_node(kind, name, name_node, node, parent_qualified_name))
    }

    fn javascript_family_symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        let (kind, name, declaration, name_node) = match node.kind() {
            "function_declaration" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("function", name, node, name_node)
            }
            "method_definition" => {
                let name_node = method_name_node(node)?;
                ("method", node_text(name_node, self.source), node, name_node)
            }
            "class_declaration" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("class", name, node, name_node)
            }
            "interface_declaration" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("interface", name, node, name_node)
            }
            "type_alias_declaration" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("type", name, node, name_node)
            }
            "enum_declaration" => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("enum", name, node, name_node)
            }
            "variable_declarator" if is_const_declarator(node, self.source) => {
                let (name, name_node) = named_child_text_and_node(node, "name", self.source)?;
                ("constant", name, declaration_parent(node), name_node)
            }
            _ => return None,
        };
        Some(self.seed_from_declaration(
            kind,
            name,
            name_node,
            declaration,
            node,
            parent_qualified_name,
        ))
    }

    fn json_symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        if node.kind() != "pair" {
            return None;
        }
        let (name, name_node) = named_child_text_and_node(node, "key", self.source)
            .map(|(key, node)| (key.trim_matches('"').to_string(), node))?;
        Some(self.seed_from_node("field", name, name_node, node, parent_qualified_name))
    }

    fn yaml_symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        if node.kind() != "block_mapping_pair" && node.kind() != "flow_pair" {
            return None;
        }
        let key = node.named_child(0)?;
        let name = node_text(key, self.source).trim_matches('"').to_string();
        if name.is_empty() {
            return None;
        }
        Some(self.seed_from_node("field", name, key, node, parent_qualified_name))
    }

    fn toml_symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        if !matches!(node.kind(), "pair" | "table" | "table_array_element") {
            return None;
        }
        let name_node = node
            .child_by_field_name("key")
            .or_else(|| node.named_child(0))?;
        let name = node_text(name_node, self.source)
            .trim_matches('"')
            .to_string();
        if name.is_empty() {
            return None;
        }
        Some(self.seed_from_node("field", name, name_node, node, parent_qualified_name))
    }

    fn markdown_symbol_seed(
        &self,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> Option<SymbolSeed> {
        if !matches!(node.kind(), "atx_heading" | "setext_heading") {
            return None;
        }
        let name = node_text(node, self.source)
            .trim_matches('#')
            .trim()
            .to_string();
        if name.is_empty() {
            return None;
        }
        Some(self.seed_from_node("heading", name, node, node, parent_qualified_name))
    }

    fn seed_from_node(
        &self,
        kind: &str,
        name: String,
        name_node: Node<'_>,
        node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> SymbolSeed {
        self.seed_from_declaration(kind, name, name_node, node, node, parent_qualified_name)
    }

    fn seed_from_declaration(
        &self,
        kind: &str,
        name: String,
        name_node: Node<'_>,
        declaration: Node<'_>,
        body_source_node: Node<'_>,
        parent_qualified_name: Option<&str>,
    ) -> SymbolSeed {
        let body = body_node(body_source_node);
        let qualified_name = qualify(parent_qualified_name, &name);
        SymbolSeed {
            kind: kind.to_string(),
            name,
            qualified_name,
            signature: Some(signature_text(declaration, body, self.source)),
            visibility: visibility(declaration, self.source, self.parsed.language),
            exported: Some(is_exported(declaration, self.source)),
            name_range: SourceRange::from_node(name_node),
            decl_range: SourceRange::from_node(declaration),
            body_range: body.map(SourceRange::from_node),
            doc_range: doc_range(declaration, self.source, self.parsed.language),
        }
    }
}

struct SymbolSeed {
    kind: String,
    name: String,
    qualified_name: String,
    signature: Option<String>,
    visibility: Option<String>,
    exported: Option<bool>,
    name_range: SourceRange,
    decl_range: SourceRange,
    body_range: Option<SourceRange>,
    doc_range: Option<SourceRange>,
}

struct ReferenceExtractor<'a> {
    parsed: &'a ParsedSource,
    source: &'a str,
    definition_names: HashSet<ReferenceKey>,
    seen: HashSet<ReferenceKey>,
    out: Vec<ExtractedReference>,
}

type ReferenceKey = (String, usize, usize, usize, usize);

impl ReferenceExtractor<'_> {
    fn visit(&mut self, node: Node<'_>) {
        if is_reference_identifier(self.parsed.language, node) {
            let name = node_text(node, self.source);
            if is_reference_name(&name) {
                let range = SourceRange::from_node(node);
                let key = reference_key(&name, range);
                if !self.definition_names.contains(&key) && self.seen.insert(key) {
                    self.out.push(ExtractedReference {
                        referenced_name: name,
                        reference_kind: reference_kind(node).to_string(),
                        range,
                    });
                }
            }
        }
        for idx in 0..node.named_child_count() {
            if let Some(child) = node.named_child(idx) {
                self.visit(child);
            }
        }
    }
}

fn reference_key(name: &str, range: SourceRange) -> ReferenceKey {
    (
        name.to_string(),
        range.start.line,
        range.start.column,
        range.end.line,
        range.end.column,
    )
}

fn is_reference_identifier(language: SourceLanguage, node: Node<'_>) -> bool {
    match language {
        SourceLanguage::Rust => matches!(
            node.kind(),
            "identifier" | "type_identifier" | "field_identifier"
        ),
        SourceLanguage::TypeScript | SourceLanguage::Tsx | SourceLanguage::JavaScript => {
            matches!(
                node.kind(),
                "identifier"
                    | "property_identifier"
                    | "shorthand_property_identifier"
                    | "type_identifier"
                    | "jsx_identifier"
            )
        }
        SourceLanguage::Json
        | SourceLanguage::Yaml
        | SourceLanguage::Toml
        | SourceLanguage::Markdown => false,
    }
}

fn is_reference_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(*byte, b'_' | b'$'))
}

fn reference_kind(node: Node<'_>) -> &'static str {
    match node.kind() {
        "field_identifier" | "property_identifier" | "shorthand_property_identifier" => "field",
        "type_identifier" => "type",
        "jsx_identifier" => "jsx",
        _ => {
            if node
                .parent()
                .is_some_and(|parent| parent.kind() == "call_expression")
            {
                "call"
            } else {
                "use"
            }
        }
    }
}

fn symbol_id(
    root_path: &str,
    relative_path: &str,
    language: SourceLanguage,
    qualified_name: &str,
    kind: &str,
    disambiguator: i32,
) -> String {
    let input = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        root_path,
        relative_path,
        language.as_str(),
        qualified_name,
        kind,
        disambiguator
    );
    let hash = digest::digest(&digest::SHA256, input.as_bytes());
    let short = hash
        .as_ref()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sym_{short}")
}

fn qualify(parent: Option<&str>, name: &str) -> String {
    match parent {
        Some(parent) if !parent.is_empty() => format!("{parent}::{name}"),
        _ => name.to_string(),
    }
}

fn named_child_text_and_node<'tree>(
    node: Node<'tree>,
    field: &str,
    source: &str,
) -> Option<(String, Node<'tree>)> {
    node.child_by_field_name(field)
        .map(|child| (node_text(child, source), child))
}

fn node_text(node: Node<'_>, source: &str) -> String {
    node.utf8_text(source.as_bytes())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn body_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("body").or_else(|| {
        (0..node.named_child_count())
            .filter_map(|idx| node.named_child(idx))
            .find(|child| {
                matches!(
                    child.kind(),
                    "block"
                        | "field_declaration_list"
                        | "declaration_list"
                        | "enum_variant_list"
                        | "class_body"
                        | "statement_block"
                        | "object"
                        | "document"
                )
            })
    })
}

fn signature_text(declaration: Node<'_>, body: Option<Node<'_>>, source: &str) -> String {
    let start = declaration.start_byte();
    let end = body
        .map(|body| body.start_byte())
        .unwrap_or_else(|| declaration.end_byte())
        .max(start);
    let raw = source.get(start..end).unwrap_or_default().trim();
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() <= MAX_SIGNATURE_BYTES {
        compact
    } else {
        format!(
            "{}...",
            compact
                .chars()
                .take(MAX_SIGNATURE_BYTES)
                .collect::<String>()
        )
    }
}

fn rust_impl_name(node: Node<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("type")
        .map(|node| format!("impl {}", node_text(node, source)))
        .or_else(|| {
            let text = node_text(node, source);
            text.split('{').next().map(str::trim).map(str::to_string)
        })
        .filter(|name| !name.is_empty())
}

fn method_name_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("property"))
}

fn declaration_parent(node: Node<'_>) -> Node<'_> {
    node.parent().unwrap_or(node)
}

fn is_const_declarator(node: Node<'_>, source: &str) -> bool {
    declaration_parent(node)
        .utf8_text(source.as_bytes())
        .unwrap_or_default()
        .trim_start()
        .starts_with("const ")
}

fn is_exported(node: Node<'_>, source: &str) -> bool {
    let mut current = Some(node);
    while let Some(node) = current {
        if node.kind() == "export_statement" {
            return true;
        }
        current = node.parent();
    }
    node_text(node, source).starts_with("pub ")
}

fn visibility(node: Node<'_>, source: &str, language: SourceLanguage) -> Option<String> {
    match language {
        SourceLanguage::Rust if node_text(node, source).starts_with("pub ") => {
            Some("public".to_string())
        }
        SourceLanguage::TypeScript | SourceLanguage::Tsx | SourceLanguage::JavaScript
            if is_exported(node, source) =>
        {
            Some("exported".to_string())
        }
        _ => None,
    }
}

fn doc_range(declaration: Node<'_>, source: &str, language: SourceLanguage) -> Option<SourceRange> {
    let prefixes = match language {
        SourceLanguage::Rust => &["///", "//!"][..],
        SourceLanguage::TypeScript | SourceLanguage::Tsx | SourceLanguage::JavaScript => {
            &["//", "*", "/**"][..]
        }
        _ => return None,
    };
    let lines = source.lines().collect::<Vec<_>>();
    let decl_line = declaration.start_position().row;
    if decl_line == 0 {
        return None;
    }
    let mut start = decl_line;
    let mut cursor = decl_line;
    while cursor > 0 {
        let prev = cursor - 1;
        let trimmed = lines.get(prev).copied().unwrap_or_default().trim_start();
        if trimmed.is_empty() {
            break;
        }
        if prefixes.iter().any(|prefix| trimmed.starts_with(prefix)) || trimmed == "*/" {
            start = prev;
            cursor = prev;
        } else {
            break;
        }
    }
    if start == decl_line {
        return None;
    }
    let end_line = decl_line;
    let end_col = lines
        .get(end_line.saturating_sub(1))
        .map(|line| line.len() + 1)
        .unwrap_or(1);
    Some(SourceRange {
        start: super::parser::SourcePosition {
            line: start + 1,
            column: 1,
        },
        end: super::parser::SourcePosition {
            line: end_line,
            column: end_col,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_intel::parser::{SourceLanguage, SourceParser};

    #[test]
    fn extracts_rust_symbols_with_stable_ids_and_ranges() {
        let source = r#"
pub struct AppState {
    value: i32,
}

impl AppState {
    /// Builds a value.
    pub fn new(value: i32) -> Self {
        Self { value }
    }
}
"#;
        let mut parser = SourceParser::default();
        let parsed = parser.parse(SourceLanguage::Rust, source).unwrap();
        let symbols = extract_symbols(&parsed, source, Path::new("/repo"), "src/lib.rs");
        let symbols_again = extract_symbols(&parsed, source, Path::new("/repo"), "src/lib.rs");

        assert_eq!(symbols, symbols_again);
        assert!(symbols
            .iter()
            .any(|symbol| symbol.kind == "struct" && symbol.name == "AppState"));
        let method = symbols
            .iter()
            .find(|symbol| symbol.kind == "function" && symbol.name == "new")
            .expect("new method");
        assert!(method.qualified_name.ends_with("new"));
        assert_eq!(method.visibility.as_deref(), Some("public"));
        assert!(method.body_range.is_some());
        assert!(method.doc_range.is_some());
        assert!(method.decl_range.start.line < method.decl_range.end.line);
    }

    #[test]
    fn extracts_syntactic_references_without_definition_names() {
        let source = r#"
pub struct AppState {
    value: i32,
}

impl AppState {
    pub fn new(value: i32) -> Self {
        AppState { value }
    }
}

fn build() {
    let state = AppState::new(1);
}
"#;
        let mut parser = SourceParser::default();
        let parsed = parser.parse(SourceLanguage::Rust, source).unwrap();
        let symbols = extract_symbols(&parsed, source, Path::new("/repo"), "src/lib.rs");
        let references = extract_references(&parsed, source, &symbols);

        assert!(references
            .iter()
            .any(|reference| reference.referenced_name == "AppState"));
        assert_eq!(
            references
                .iter()
                .filter(|reference| reference.referenced_name == "new")
                .count(),
            1
        );
    }

    #[test]
    fn extracts_typescript_family_symbols() {
        let source = r#"
export class Widget {
  render() {
    return null;
  }
}

const answer = 42;
"#;
        let mut parser = SourceParser::default();
        let parsed = parser.parse(SourceLanguage::TypeScript, source).unwrap();
        let symbols = extract_symbols(&parsed, source, Path::new("/repo"), "src/widget.ts");

        assert!(symbols
            .iter()
            .any(|symbol| symbol.kind == "class" && symbol.name == "Widget"));
        assert!(symbols
            .iter()
            .any(|symbol| symbol.kind == "method" && symbol.name == "render"));
        assert!(symbols
            .iter()
            .any(|symbol| symbol.kind == "constant" && symbol.name == "answer"));
    }
}
