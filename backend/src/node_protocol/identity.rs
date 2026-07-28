//! The control plane's own signing identity.
//!
//! The node handshake proves who the *node* is. This is the other direction:
//! it lets a node prove it is still talking to the control plane that first
//! paired it, rather than to anything else that answers on the same address.
//!
//! A node pins this public key the first time it pairs successfully and refuses
//! every later connection that cannot sign for it. First pairing is therefore
//! the one moment a node can be captured — an accepted trade, because the
//! machine is being installed by hand at that point anyway.

use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};

use super::model::{ControlChallenge, ControlHelloProof, NodeHello};
use super::NodeProtocolError;
use crate::db::Pool;

pub struct ControlIdentity {
    key: Ed25519KeyPair,
    public_key: String,
}

impl ControlIdentity {
    /// Loads the stored identity, generating one on first start.
    ///
    /// Two control processes racing on a fresh database both insert; the
    /// conflict clause makes the loser adopt the winner's key rather than
    /// leaving half the nodes pinned to a key that no longer exists.
    pub async fn load_or_create(pool: &Pool) -> Result<Self, NodeProtocolError> {
        if let Some(stored) = Self::stored(pool).await? {
            return Ok(stored);
        }
        let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
            .map_err(|_| NodeProtocolError::Cryptography("control key generation failed".into()))?;
        sqlx::query(
            "INSERT INTO control_identity (id, credential_kind, private_key) \
             VALUES (1, 'ed25519', $1) ON CONFLICT (id) DO NOTHING",
        )
        .bind(document.as_ref())
        .execute(pool)
        .await?;
        Self::stored(pool)
            .await?
            .ok_or_else(|| NodeProtocolError::Cryptography("control identity vanished".into()))
    }

    async fn stored(pool: &Pool) -> Result<Option<Self>, NodeProtocolError> {
        let Some((private_key,)) = sqlx::query_as::<_, (Vec<u8>,)>(
            "SELECT private_key FROM control_identity WHERE id = 1",
        )
        .fetch_optional(pool)
        .await?
        else {
            return Ok(None);
        };
        Ok(Some(Self::from_pkcs8(&private_key)?))
    }

    pub fn from_pkcs8(document: &[u8]) -> Result<Self, NodeProtocolError> {
        let key = Ed25519KeyPair::from_pkcs8(document)
            .map_err(|_| NodeProtocolError::Cryptography("stored control key is invalid".into()))?;
        let public_key =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.public_key().as_ref());
        Ok(Self { key, public_key })
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    /// Signs the handshake so the node can bind this connection to this key.
    ///
    /// The node's own nonce is covered, so a proof captured from an earlier
    /// connection cannot be replayed into a later one. The TLS certificate
    /// digest is covered so the encrypted transport the node sees is bound to
    /// this identity.
    pub fn prove_handshake(
        &self,
        challenge: &ControlChallenge,
        hello: &NodeHello,
        tls_cert_digest: Option<&str>,
    ) -> ControlHelloProof {
        let mut proof = ControlHelloProof {
            public_key: self.public_key.clone(),
            tls_cert_digest: tls_cert_digest.map(str::to_string),
            signature: String::new(),
        };
        proof.signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            self.key
                .sign(&proof.signing_payload(challenge, hello))
                .as_ref(),
        );
        proof
    }

    /// Signs a configuration digest, bound to the node nonce it is being sent
    /// to. Without this an on-path attacker could replace the payload on an
    /// otherwise authenticated connection.
    pub fn sign_config(&self, digest: &str, node_nonce: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            self.key
                .sign(&super::model::config_signing_payload(digest, node_nonce))
                .as_ref(),
        )
    }
}

impl std::fmt::Debug for ControlIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ControlIdentity")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}
