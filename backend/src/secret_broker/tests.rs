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
fn validation_rejects_secret_ids_that_cannot_be_path_or_query_safe() {
    assert!(validate_secret_id("anthropic.api-key_1").is_ok());

    for id in ["", "../aws", "aws default", "aws/default"] {
        let err = validate_secret_id(id).expect_err("invalid secret id");
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
    }
}
