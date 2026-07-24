use super::*;

const FAKE_LSP_SCRIPT: &str = r#"
starts="$(pwd)/.fake_lsp_starts"
count="$(cat "$starts" 2>/dev/null || echo 0)"
count=$((count + 1))
printf '%s\n' "$count" > "$starts"
send() {
  body="$1"
  len="$(printf '%s' "$body" | wc -c | tr -d ' ')"
  printf 'Content-Length: %s\r\n\r\n%s' "$len" "$body"
}
while IFS= read -r header; do
  len="$(printf '%s' "$header" | tr -dc '0-9')"
  IFS= read -r blank || exit 0
  body="$(dd bs=1 count="$len" 2>/dev/null)"
  id="$(printf '%s' "$body" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')"
  case "$body" in
*'"method":"initialize"'*)
  send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":{\"capabilities\":{}}}"
  ;;
*'"method":"textDocument/definition"'*)
  send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":{\"uri\":\"file://$(pwd)/src/main.ts\",\"range\":{\"start\":{\"line\":0,\"character\":6},\"end\":{\"line\":0,\"character\":11}}}}"
  ;;
*'"method":"textDocument/references"'*)
  send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":[{\"uri\":\"file://$(pwd)/src/main.ts\",\"range\":{\"start\":{\"line\":0,\"character\":6},\"end\":{\"line\":0,\"character\":11}}}]}"
  ;;
*'"id":'*)
  send "{\"jsonrpc\":\"2.0\",\"id\":${id:-1},\"result\":null}"
  ;;
  esac
done
"#;

#[test]
fn parses_location_and_location_link_results() {
    let direct = json!({
        "uri": "file:///repo/src/lib.rs",
        "range": {
            "start": { "line": 4, "character": 8 },
            "end": { "line": 4, "character": 14 }
        }
    });
    let link = json!({
        "targetUri": "file:///repo/src/main.rs",
        "targetSelectionRange": {
            "start": { "line": 1, "character": 0 },
            "end": { "line": 1, "character": 4 }
        }
    });

    assert_eq!(
        definition_locations(&json!([direct, link])),
        vec![
            RawLocation {
                uri: "file:///repo/src/lib.rs".to_string(),
                start_line: 5,
                start_col: 9,
                end_line: 5,
                end_col: 15,
            },
            RawLocation {
                uri: "file:///repo/src/main.rs".to_string(),
                start_line: 2,
                start_col: 1,
                end_line: 2,
                end_col: 5,
            },
        ]
    );
}

#[test]
fn resolves_file_uri_locations_under_root() {
    let locations = resolve_locations(
        Path::new("/repo"),
        vec![
            RawLocation {
                uri: "file:///elsewhere/lib.rs".to_string(),
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 2,
            },
            RawLocation {
                uri: "file:///repo/src/lib.rs".to_string(),
                start_line: 1,
                start_col: 1,
                end_line: 1,
                end_col: 2,
            },
        ],
    );

    assert_eq!(
        locations,
        vec![LspLocation {
            path: "src/lib.rs".to_string(),
            start_line: 1,
            start_col: 1,
            end_line: 1,
            end_col: 2,
        }]
    );
}

#[test]
fn command_detection_uses_explicit_path_list() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("rust-analyzer");
    std::fs::write(&executable, "#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    assert!(command_available_in_paths(
        "rust-analyzer",
        &[temp.path().to_path_buf()]
    ));
    assert!(!command_available_in_paths(
        "typescript-language-server",
        &[temp.path().to_path_buf()]
    ));
}

#[test]
fn converts_one_based_byte_column_to_lsp_utf16_character() {
    let source = "fn main() {}\nlet name = \"value\";\nlet emoji = \"🚀\";\n";

    assert_eq!(
        utf16_character_for_one_based_byte_col(source, 2, 5),
        4,
        "ASCII column maps directly"
    );
    assert_eq!(
        utf16_character_for_one_based_byte_col(source, 3, 14),
        13,
        "columns before non-BMP text are unaffected"
    );
    assert_eq!(
        utf16_character_for_one_based_byte_col("let café_value = 1;\n", 1, 10),
        8,
        "multi-byte UTF-8 before the cursor is counted as UTF-16"
    );
}

#[test]
fn workspace_configuration_response_matches_requested_item_count() {
    let response = workspace_configuration_result(&json!({
        "params": {
            "items": [{ "section": "rust-analyzer" }, { "section": "typescript" }]
        }
    }));
    assert_eq!(response, json!([{}, {}]));
}

#[test]
fn javascript_language_id_distinguishes_jsx_files() {
    let spec = ServerSpec::for_language(SourceLanguage::JavaScript).unwrap();

    assert_eq!(
        spec.language_id_for_path(Path::new("src/app.ts")),
        "typescript"
    );
    assert_eq!(
        spec.language_id_for_path(Path::new("src/app.tsx")),
        "typescriptreact"
    );
    assert_eq!(
        spec.language_id_for_path(Path::new("src/app.js")),
        "javascript"
    );
    assert_eq!(
        spec.language_id_for_path(Path::new("src/app.jsx")),
        "javascriptreact"
    );
}

#[test]
fn typescript_family_shares_one_server_key_per_root() {
    let root = CodeRootSpec {
        kind: super::super::indexer::CodeRootKind::Repo,
        name: "repo".to_string(),
        path: PathBuf::from("/repo"),
        repo_name: Some("repo".to_string()),
        workspace_id: None,
        git_head: None,
    };

    let ts_key = LspClientKey::new(
        &root,
        ServerSpec::for_language(SourceLanguage::TypeScript).unwrap(),
    );
    let tsx_key = LspClientKey::new(
        &root,
        ServerSpec::for_language(SourceLanguage::Tsx).unwrap(),
    );
    let js_key = LspClientKey::new(
        &root,
        ServerSpec::for_language(SourceLanguage::JavaScript).unwrap(),
    );
    let rust_key = LspClientKey::new(
        &root,
        ServerSpec::for_language(SourceLanguage::Rust).unwrap(),
    );

    assert_eq!(ts_key, tsx_key);
    assert_eq!(ts_key, js_key);
    assert_ne!(ts_key, rust_key);
}

#[tokio::test]
async fn root_language_server_reuses_one_process_for_repeated_requests() {
    let temp = tempfile::tempdir().unwrap();
    let root_path = temp.path();
    std::fs::create_dir(root_path.join("src")).unwrap();
    let file_path = root_path.join("src/main.ts");
    let source = "const value = 1;\n";
    std::fs::write(&file_path, source).unwrap();
    let root = CodeRootSpec {
        kind: super::super::indexer::CodeRootKind::Repo,
        name: "repo".to_string(),
        path: root_path.to_path_buf(),
        repo_name: Some("repo".to_string()),
        workspace_id: None,
        git_head: None,
    };
    let spec = ServerSpec {
        language: "typescript",
        server_family: "typescript",
        source_languages: &[SourceLanguage::TypeScript],
        command: "/bin/sh",
        args: &["-c", FAKE_LSP_SCRIPT],
        required_commands: &[],
        language_id: "typescript",
    };
    let mut server = RootLanguageServer::spawn(spec, &root, Duration::from_secs(1))
        .await
        .unwrap();

    let first = server
        .request_locations(LspLocationRequest {
            file_path: &file_path,
            source,
            line: 1,
            col: 7,
            kind: LspRequestKind::Definition,
            request_timeout: Duration::from_secs(1),
            warmup_timeout: Duration::from_secs(1),
        })
        .await
        .unwrap();
    let second = server
        .request_locations(LspLocationRequest {
            file_path: &file_path,
            source,
            line: 1,
            col: 7,
            kind: LspRequestKind::Definition,
            request_timeout: Duration::from_secs(1),
            warmup_timeout: Duration::from_secs(1),
        })
        .await
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(first[0].path, "src/main.ts");
    assert_eq!(
        std::fs::read_to_string(root_path.join(".fake_lsp_starts"))
            .unwrap()
            .trim(),
        "1"
    );
}
