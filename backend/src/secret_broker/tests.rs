use super::*;

#[test]
fn secret_crypto_round_trips_encrypted_env_payloads() {
    let key = Key::from_slice(&[7_u8; 32]);
    let crypto = SecretCrypto {
        cipher: ChaCha20Poly1305::new(key),
    };
    let env = HashMap::from([
        ("ANTHROPIC_API_KEY".to_string(), "sk-ant-test".to_string()),
        ("CLAUDE_API_KEY".to_string(), "claude-test".to_string()),
        (
            "SSH_PRIVATE_KEY".to_string(),
            "-----BEGIN OPENSSH PRIVATE KEY-----\nZmFrZQ==\n-----END OPENSSH PRIVATE KEY-----\n"
                .to_string(),
        ),
    ]);

    let (ciphertext, nonce) = crypto.encrypt_env(&env).expect("encrypt env");

    assert_eq!(nonce.len(), 12);
    assert_ne!(ciphertext, serde_json::to_vec(&env).expect("serialize env"));
    assert_eq!(
        crypto
            .decrypt_env(&ciphertext, &nonce)
            .expect("decrypt env"),
        env
    );
}

#[test]
fn validation_allows_only_supported_secret_tools() {
    assert!(validate_tool_name("with-cred").is_ok());
    assert!(validate_tool_name("aws").is_ok());

    let err = validate_tool_name("shell").expect_err("unsupported tool");
    assert_eq!(err.status, StatusCode::BAD_REQUEST);
}

#[test]
fn validation_rejects_secret_ids_that_cannot_be_path_or_query_safe() {
    assert!(validate_secret_id("anthropic.api-key_1").is_ok());

    for id in ["", "../aws", "aws default", "aws/default"] {
        let err = validate_secret_id(id).expect_err("invalid secret id");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }
}

#[test]
fn unnamed_aws_redemption_matches_aws_shaped_env_bundle() {
    let aws_env = HashMap::from([
        ("AWS_ACCESS_KEY_ID".to_string(), "AKIA...".to_string()),
        ("AWS_SECRET_ACCESS_KEY".to_string(), "secret".to_string()),
    ]);
    let unrelated_env = HashMap::from([("OPENAI_API_KEY".to_string(), "sk-test".to_string())]);

    assert!(secret_matches_redemption("aws", None, &aws_env));
    assert!(!secret_matches_redemption("aws", None, &unrelated_env));
    assert!(secret_matches_redemption("with-cred", None, &unrelated_env));
    assert!(secret_matches_redemption(
        "aws",
        Some("explicit-secret"),
        &unrelated_env
    ));
}

#[test]
fn unnamed_aws_redemption_has_specific_forbidden_message() {
    assert_eq!(
        redemption_forbidden_message("aws", None),
        "no AWS credential is enabled for this terminal"
    );
    assert_eq!(
        redemption_forbidden_message("with-cred", None),
        "secret is not unlocked for this terminal"
    );
    assert_eq!(
        redemption_forbidden_message("aws", Some("AWS")),
        "secret is not unlocked for this terminal"
    );
}
