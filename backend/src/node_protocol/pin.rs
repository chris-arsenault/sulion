//! The node's record of which control plane paired it.
//!
//! First pairing is trust-on-first-use: a freshly installed node accepts
//! whichever control plane answers, because it has nothing to compare against
//! and the machine is being installed by hand at that moment anyway. What it
//! learns then it keeps. Every later connection must be signed by that same
//! identity, so a machine that answers on the right address later — after a
//! rebuild, a re-address, or an attacker taking the IP — is refused.
//!
//! The pin lives in root-owned host state beside the node's own identity key,
//! so it survives container replacement, release upgrades, and `nixos-rebuild`.

use std::path::{Path, PathBuf};

use base64::Engine;
use ring::signature;

use super::model::{ControlChallenge, ControlHelloProof, NodeHello};

pub const PIN_PATH_ENV: &str = "SULION_NODE_CONTROL_PIN_PATH";
pub const DEFAULT_PIN_PATH: &str = "/var/lib/sulion-node/control-key.pub";

/// What verifying a connection's proof established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinOutcome {
    /// Matched the key this node already pinned.
    Matched,
    /// First pairing: this key should now be pinned.
    FirstPairing(String),
    /// The control plane offered no identity and this node has pinned none.
    /// Only reachable before a node has ever paired with a signing control
    /// plane; once pinned, a missing proof is a refusal.
    Unauthenticated,
}

pub struct ControlPin {
    path: PathBuf,
}

impl ControlPin {
    pub fn from_env() -> Self {
        Self::new(PathBuf::from(
            std::env::var(PIN_PATH_ENV).unwrap_or_else(|_| DEFAULT_PIN_PATH.to_string()),
        ))
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pinned control public key, if this node has paired before.
    pub fn pinned(&self) -> Option<String> {
        let pinned = std::fs::read_to_string(&self.path).ok()?;
        let pinned = pinned.trim().to_string();
        (!pinned.is_empty()).then_some(pinned)
    }

    /// Checks a connection's proof against the pin.
    ///
    /// A pinned node refuses a missing proof as firmly as a wrong one, so the
    /// signature cannot simply be stripped to force a downgrade.
    pub fn verify(
        &self,
        proof: Option<&ControlHelloProof>,
        challenge: &ControlChallenge,
        hello: &NodeHello,
    ) -> anyhow::Result<PinOutcome> {
        let pinned = self.pinned();
        let Some(proof) = proof else {
            if pinned.is_some() {
                anyhow::bail!(
                    "control plane offered no identity but this node is paired to one; \
                     refusing to continue"
                );
            }
            return Ok(PinOutcome::Unauthenticated);
        };

        verify_proof(proof, challenge, hello)?;

        match pinned {
            Some(pinned) if pinned == proof.public_key => Ok(PinOutcome::Matched),
            Some(pinned) => anyhow::bail!(
                "control plane identity changed: this node is paired to {pinned} but the \
                 peer proved {}; refusing to continue",
                proof.public_key
            ),
            None => Ok(PinOutcome::FirstPairing(proof.public_key.clone())),
        }
    }

    /// Records the pinned key. Written once at first pairing; recovering from a
    /// legitimately rotated control plane means deleting this file deliberately.
    pub fn record(&self, public_key: &str) -> anyhow::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = self.path.with_extension("pin.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        writeln!(file, "{public_key}")?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &self.path)?;
        Ok(())
    }

    /// Verifies a delivered configuration against the pinned identity.
    ///
    /// The handshake proof authenticates the peer, but the channel carrying the
    /// payload is not integrity-protected on its own, so the payload is signed
    /// separately and bound to this connection's nonce.
    pub fn verify_config(
        &self,
        digest: &str,
        node_nonce: &str,
        signature: Option<&str>,
    ) -> anyhow::Result<()> {
        let Some(pinned) = self.pinned() else {
            return Ok(());
        };
        let Some(signature) = signature else {
            anyhow::bail!("delivered configuration is unsigned but this node is paired");
        };
        let public_key = decode(&pinned)?;
        let signature = decode(signature)?;
        signature::UnparsedPublicKey::new(&signature::ED25519, &public_key)
            .verify(
                &super::model::config_signing_payload(digest, node_nonce),
                &signature,
            )
            .map_err(|_| anyhow::anyhow!("delivered configuration signature does not verify"))
    }
}

fn verify_proof(
    proof: &ControlHelloProof,
    challenge: &ControlChallenge,
    hello: &NodeHello,
) -> anyhow::Result<()> {
    let public_key = decode(&proof.public_key)?;
    let signature = proof.decode_signature()?;
    signature::UnparsedPublicKey::new(&signature::ED25519, &public_key)
        .verify(&proof.signing_payload(challenge, hello), &signature)
        .map_err(|_| anyhow::anyhow!("control plane proof does not verify"))
}

fn decode(value: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|err| anyhow::anyhow!("invalid base64 in control identity: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_protocol::identity::ControlIdentity;
    use ring::signature::Ed25519KeyPair;
    use uuid::Uuid;

    fn identity() -> ControlIdentity {
        let document = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        ControlIdentity::from_pkcs8(document.as_ref()).unwrap()
    }

    fn challenge() -> ControlChallenge {
        ControlChallenge {
            challenge_id: Uuid::new_v4(),
            nonce: "control-nonce".into(),
            protocol_version: 1,
        }
    }

    fn hello() -> NodeHello {
        NodeHello {
            node_id: Uuid::new_v4(),
            boot_id: Uuid::new_v4(),
            protocol_version: 1,
            public_key: None,
            node_nonce: "node-nonce".into(),
            signature: String::new(),
        }
    }

    fn pin(directory: &tempfile::TempDir) -> ControlPin {
        ControlPin::new(directory.path().join("control-key.pub"))
    }

    #[test]
    fn first_pairing_adopts_the_key_and_later_connections_match_it() {
        let directory = tempfile::tempdir().unwrap();
        let pin = pin(&directory);
        let control = identity();
        let (challenge, hello) = (challenge(), hello());

        let proof = control.prove_handshake(&challenge, &hello, None);
        let outcome = pin.verify(Some(&proof), &challenge, &hello).unwrap();
        assert_eq!(
            outcome,
            PinOutcome::FirstPairing(control.public_key().to_string())
        );
        pin.record(control.public_key()).unwrap();

        let proof = control.prove_handshake(&challenge, &hello, None);
        assert_eq!(
            pin.verify(Some(&proof), &challenge, &hello).unwrap(),
            PinOutcome::Matched
        );
    }

    #[test]
    fn a_different_control_plane_is_refused_after_pairing() {
        let directory = tempfile::tempdir().unwrap();
        let pin = pin(&directory);
        let (challenge, hello) = (challenge(), hello());
        pin.record(identity().public_key()).unwrap();

        // A well-formed proof from a key this node never paired with.
        let impostor = identity();
        let proof = impostor.prove_handshake(&challenge, &hello, None);
        let error = pin.verify(Some(&proof), &challenge, &hello).unwrap_err();
        assert!(error.to_string().contains("identity changed"));
    }

    #[test]
    fn a_paired_node_refuses_a_stripped_proof() {
        let directory = tempfile::tempdir().unwrap();
        let pin = pin(&directory);
        pin.record(identity().public_key()).unwrap();

        let error = pin.verify(None, &challenge(), &hello()).unwrap_err();
        assert!(error.to_string().contains("offered no identity"));
    }

    #[test]
    fn a_proof_from_another_connection_does_not_replay() {
        let directory = tempfile::tempdir().unwrap();
        let pin = pin(&directory);
        let control = identity();
        pin.record(control.public_key()).unwrap();

        let first = challenge();
        let proof = control.prove_handshake(&first, &hello(), None);
        // Same key, but a different connection: the nonces no longer match.
        let error = pin
            .verify(Some(&proof), &challenge(), &hello())
            .unwrap_err();
        assert!(error.to_string().contains("does not verify"));
    }

    #[test]
    fn an_unpaired_node_accepts_a_control_plane_with_no_identity() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            pin(&directory)
                .verify(None, &challenge(), &hello())
                .unwrap(),
            PinOutcome::Unauthenticated
        );
    }

    #[test]
    fn a_paired_node_verifies_the_configuration_signature() {
        let directory = tempfile::tempdir().unwrap();
        let pin = pin(&directory);
        let control = identity();
        pin.record(control.public_key()).unwrap();

        let signature = control.sign_config("digest", "node-nonce");
        pin.verify_config("digest", "node-nonce", Some(&signature))
            .expect("valid signature");

        // Tampering with the values changes the digest, which the signature covers.
        assert!(pin
            .verify_config("other-digest", "node-nonce", Some(&signature))
            .is_err());
        // And it cannot be lifted onto another connection.
        assert!(pin
            .verify_config("digest", "other-nonce", Some(&signature))
            .is_err());
        assert!(pin.verify_config("digest", "node-nonce", None).is_err());
    }

    #[test]
    fn the_pin_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let pin = pin(&directory);
        pin.record(identity().public_key()).unwrap();
        let mode = std::fs::metadata(pin.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0);
    }
}
