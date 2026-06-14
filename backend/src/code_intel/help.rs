use serde::Serialize;

pub const HELP_TEXT: &str = r#"Sulion code intelligence

Usage:
  sulion-code [--json] [--budget small|normal|large] <command> ...

Commands:
  help
  status
  index-status
  refresh [path]
  outline [path]
  find <symbol-or-name>
  def <path:line[:col] | symbol-id>
  refs <path:line[:col] | symbol-id>
  search <lang> <pattern> [path]
  patch <lang> <pattern> <rewrite> [path]
  pack <path:line-line | symbol-id>

Rules:
  scope is inferred from cwd; cd to change root
  patch returns a unified diff only
  def/refs try semantic resolution, then syntactic fallback

Start:
  sulion-code help
  sulion-code status
  sulion-code outline .
  sulion-code find <symbol-or-name>"#;

pub const OPTIONS: &[&str] = &["--json", "--budget small|normal|large"];

pub const RULES: &[&str] = &[
    "scope is inferred from cwd; cd to change root",
    "patch returns a unified diff only",
    "def/refs try semantic resolution, then syntactic fallback",
];

pub const EXAMPLES: &[&str] = &[
    "sulion-code help",
    "sulion-code status",
    "sulion-code outline .",
    "sulion-code find <symbol-or-name>",
];

pub const COMMANDS: &[HelpCommand] = &[
    HelpCommand {
        name: "help",
        usage: "sulion-code help",
        summary: "print this concise command reference",
    },
    HelpCommand {
        name: "status",
        usage: "sulion-code status",
        summary: "show root, index freshness, languages, and semantic availability",
    },
    HelpCommand {
        name: "index-status",
        usage: "sulion-code index-status",
        summary: "show index backlog and latest indexing job for the current root",
    },
    HelpCommand {
        name: "refresh",
        usage: "sulion-code refresh [path]",
        summary: "mark the current root or path dirty for background indexing",
    },
    HelpCommand {
        name: "outline",
        usage: "sulion-code outline [path]",
        summary: "list structural symbols for a file or directory",
    },
    HelpCommand {
        name: "find",
        usage: "sulion-code find <symbol-or-name>",
        summary: "find symbols by name",
    },
    HelpCommand {
        name: "def",
        usage: "sulion-code def <path:line[:col] | symbol-id>",
        summary: "find a definition with semantic escalation where available",
    },
    HelpCommand {
        name: "refs",
        usage: "sulion-code refs <path:line[:col] | symbol-id>",
        summary: "find references with semantic escalation where available",
    },
    HelpCommand {
        name: "search",
        usage: "sulion-code search <lang> <pattern> [path]",
        summary: "run structural ast-grep search",
    },
    HelpCommand {
        name: "patch",
        usage: "sulion-code patch <lang> <pattern> <rewrite> [path]",
        summary: "return a unified diff for a structural rewrite",
    },
    HelpCommand {
        name: "pack",
        usage: "sulion-code pack <path:line-line | symbol-id>",
        summary: "return a budgeted context bundle",
    },
];

#[derive(Debug, Serialize)]
pub struct HelpResponse {
    pub schema_version: u32,
    pub command: &'static str,
    pub usage: &'static str,
    pub options: &'static [&'static str],
    pub commands: &'static [HelpCommand],
    pub rules: &'static [&'static str],
    pub examples: &'static [&'static str],
}

#[derive(Debug, Serialize)]
pub struct HelpCommand {
    pub name: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
}

pub fn help_response(schema_version: u32) -> HelpResponse {
    HelpResponse {
        schema_version,
        command: "help",
        usage: "sulion-code [--json] [--budget small|normal|large] <command> ...",
        options: OPTIONS,
        commands: COMMANDS,
        rules: RULES,
        examples: EXAMPLES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_text_is_compact_and_teaches_one_entrypoint() {
        assert!(HELP_TEXT.contains("sulion-code help"));
        assert!(HELP_TEXT.contains("patch returns a unified diff only"));
        assert!(HELP_TEXT.lines().count() <= 30);
        assert!(!HELP_TEXT.contains("sulion code "));
    }

    #[test]
    fn help_response_matches_text_contract() {
        let response = help_response(1);
        assert_eq!(response.command, "help");
        assert!(response
            .commands
            .iter()
            .any(|command| command.name == "pack"));
        assert_eq!(response.options, OPTIONS);
    }
}
