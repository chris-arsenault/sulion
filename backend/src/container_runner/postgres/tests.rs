use super::*;
use std::path::PathBuf;

#[test]
fn parses_default_restart_temp_and_clean_cli() {
    let action = parse_postgres_cli_args(&[OsString::from("--"), OsString::from("cargo")]).unwrap();
    assert!(matches!(
        action,
        PostgresCliAction::Run {
            mode: PostgresMode::Reuse,
            ..
        }
    ));

    let action = parse_postgres_cli_args(&[
        OsString::from("--restart"),
        OsString::from("--"),
        OsString::from("cargo"),
    ])
    .unwrap();
    assert!(matches!(
        action,
        PostgresCliAction::Run {
            mode: PostgresMode::Restart,
            ..
        }
    ));

    let action = parse_postgres_cli_args(&[
        OsString::from("--temp"),
        OsString::from("--"),
        OsString::from("cargo"),
    ])
    .unwrap();
    assert!(matches!(
        action,
        PostgresCliAction::Run {
            mode: PostgresMode::Temp,
            ..
        }
    ));

    assert!(matches!(
        parse_postgres_cli_args(&[OsString::from("clean")]).unwrap(),
        PostgresCliAction::Clean
    ));
}

#[test]
fn rejects_confusing_cli_combinations() {
    assert!(
        parse_postgres_cli_args(&[OsString::from("--restart"), OsString::from("--temp")]).is_err()
    );
    assert!(parse_postgres_cli_args(&[OsString::from("--")]).is_err());
    assert!(parse_postgres_cli_args(&[OsString::from("cargo")]).is_err());
}

#[test]
fn workspace_postgres_key_is_stable_and_container_safe() {
    assert_eq!(
        postgres_key(
            Some("18bd1823-5ed0-4160-85c5-c8a7a3e03d7b"),
            &PathBuf::from("/home/dev/repos/sulion")
        ),
        "ws-18bd1823-5ed0-4160-85c5-c8a7a3e03d7b"
    );
    assert_eq!(
        postgres_key(None, &PathBuf::from("/home/dev/repos/sulion")),
        postgres_key(None, &PathBuf::from("/home/dev/repos/sulion"))
    );
}

#[test]
fn database_url_escapes_userinfo() {
    assert_eq!(encode_userinfo("p@ss word"), "p%40ss%20word");
}
