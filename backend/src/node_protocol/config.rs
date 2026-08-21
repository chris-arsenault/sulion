//! Runtime configuration delivered to a paired development node.
//!
//! The dedicated node holds no secrets before it is approved. Its Ed25519
//! identity plus the operator's approval is the whole bootstrap; everything
//! else the node needs to run arrives over that authenticated channel and is
//! written to root-owned host state by the node itself.
//!
//! Only the keys in [`FORWARDED_KEYS`] cross this boundary. The control plane
//! never forwards its own environment wholesale, so adding an unrelated
//! variable to the control container cannot leak it to a node.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use base64::Engine;
use ring::digest;

use super::model::NodeConfigPayload;

/// Digest of the configuration the node's own environment was built from.
/// Written into the host runtime environment by the activation step, so the
/// node can tell "this is already applied" from "the host must reactivate".
pub const CONFIG_DIGEST_ENV: &str = "SULION_NODE_CONFIG_DIGEST";
/// Where the node writes what it was given. Root-owned host state, mounted
/// into the node container; a host path unit watches it.
pub const DELIVERED_PATH_ENV: &str = "SULION_NODE_DELIVERED_CONFIG_PATH";
pub const DEFAULT_DELIVERED_PATH: &str = "/var/lib/sulion-node/delivered.env";

/// Control-plane environment variables forwarded to an approved node.
///
/// This channel is the node's only source of shared credentials. A node runs
/// on its own host with no AWS identity, so it cannot read them for itself the
/// way a TrueNAS workload does; the control plane reads them with its identity
/// and hands over exactly this list.
///
/// The database URL crosses whole rather than as user and password. Nothing on
/// the node assembles a connection string: Compose expands its variables before
/// a container runs, and the node's own env file is written after delivery, so
/// a DSN built from parts would have to be built in one of those two places and
/// neither can see the credential at the time it would need it.
///
/// The broker master key and Cognito credentials are deliberately absent: they
/// stay on TrueNAS and no node role reads them. So is the code-intelligence
/// token, whose two ends both live on the node's own loopback — that host
/// generates its own rather than sharing the control plane's.
const FORWARDED_KEYS: &[&str] = &[
    "SULION_DB_URL",
    "SULION_RETRIEVAL_TOKEN",
    "SULION_SECRET_BROKER_REGISTRATION_TOKEN",
];

/// Values the control plane hands to an approved node, plus the digest the
/// node uses to decide whether its own environment already reflects them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRuntimeConfig {
    digest: String,
    values: BTreeMap<String, String>,
}

impl NodeRuntimeConfig {
    /// Collects the forwarded keys from the process environment.
    ///
    /// Returns `None` when the control plane carries none of them, which keeps
    /// standalone and test deployments from advertising an empty configuration.
    /// Values that cannot survive the env-file round trip are a startup error
    /// rather than a silently dropped credential.
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let mut values = BTreeMap::new();
        for key in FORWARDED_KEYS {
            let Ok(value) = std::env::var(key) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            if !is_env_file_safe(&value) {
                anyhow::bail!(
                    "{key} cannot be delivered to a node: values must not contain \
                     newlines or single quotes"
                );
            }
            values.insert((*key).to_string(), value);
        }
        Ok((!values.is_empty()).then(|| Self::new(values)))
    }

    pub fn new(values: BTreeMap<String, String>) -> Self {
        let digest = digest_of(&values);
        Self { digest, values }
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Names of the forwarded keys. Safe to log; the values are not.
    pub fn key_names(&self) -> Vec<&str> {
        self.values.keys().map(String::as_str).collect()
    }

    pub fn payload(&self) -> NodeConfigPayload {
        NodeConfigPayload {
            digest: self.digest.clone(),
            values: self.values.clone(),
            signature: None,
        }
    }

    /// True when the environment this process already runs with was built from
    /// this exact configuration, i.e. the host has activated it.
    pub fn matches_current_env(&self) -> bool {
        std::env::var(CONFIG_DIGEST_ENV).is_ok_and(|current| current == self.digest)
    }

    /// Env-file rendering consumed by both `docker compose --env-file` and
    /// systemd `EnvironmentFile`. Values are single-quoted, which both parsers
    /// unquote, and [`is_env_file_safe`] has already excluded the characters
    /// that quoting could not carry.
    pub fn render_env_file(&self) -> String {
        let mut rendered = String::from(
            "# Generated by sulion-node from the control plane. Do not edit.\n\
             # Delivered over the authenticated node channel after approval.\n",
        );
        for (key, value) in &self.values {
            rendered.push_str(&format!("{key}='{value}'\n"));
        }
        rendered.push_str(&format!("{CONFIG_DIGEST_ENV}='{}'\n", self.digest));
        rendered
    }

    /// Path the node writes delivered configuration to.
    pub fn delivered_path() -> PathBuf {
        PathBuf::from(
            std::env::var(DELIVERED_PATH_ENV)
                .unwrap_or_else(|_| DEFAULT_DELIVERED_PATH.to_string()),
        )
    }

    /// Writes the configuration if it differs from what is already on disk.
    ///
    /// Returns whether the file changed, so the caller can stay quiet when a
    /// reconnect re-delivers the configuration the host already activated.
    /// The write is atomic because a host path unit watches this file and must
    /// never observe a partial env.
    pub fn write_delivered(&self, path: &Path) -> anyhow::Result<bool> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let rendered = self.render_env_file();
        if std::fs::read_to_string(path).is_ok_and(|current| current == rendered) {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("delivered.tmp");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(rendered.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)?;
        Ok(true)
    }
}

/// Rejects values that a quoted env-file line could not carry back intact.
fn is_env_file_safe(value: &str) -> bool {
    !value.contains('\'') && !value.contains('\n') && !value.contains('\r')
}

impl NodeRuntimeConfig {
    /// Accepts a delivered payload, enforcing the same allowlist the sending
    /// side applies.
    ///
    /// The receiving check is the one that matters. What lands here is written
    /// to a file consumed both as a Compose `--env-file` and as `EnvironmentFile=`
    /// for a root systemd unit, where Compose interpolates it into image
    /// references, bind-mount sources, and the privilege-drop identity. Taking
    /// the map verbatim would let whatever is on the other end of the socket
    /// choose those, so an unexpected key is a refusal rather than a filtered
    /// value: it means the peer is not the control plane this node expects.
    pub fn accept(payload: NodeConfigPayload) -> anyhow::Result<Self> {
        for key in payload.values.keys() {
            if !FORWARDED_KEYS.contains(&key.as_str()) {
                anyhow::bail!("refusing node configuration containing unexpected key {key}");
            }
        }
        for (key, value) in &payload.values {
            if !is_env_file_safe(value) {
                anyhow::bail!("refusing node configuration: {key} is not env-file safe");
            }
        }
        let accepted = Self::new(payload.values);
        if accepted.digest != payload.digest {
            anyhow::bail!("delivered node configuration digest does not match its values");
        }
        Ok(accepted)
    }
}

/// Digest over the canonical `key=value` rendering of the delivered map.
///
/// The node compares this against the digest baked into its own environment by
/// the host activation step, so it must not depend on map iteration order.
fn digest_of(values: &BTreeMap<String, String>) -> String {
    let mut context = digest::Context::new(&digest::SHA256);
    for (key, value) in values {
        context.update(key.as_bytes());
        context.update(b"=");
        context.update(value.as_bytes());
        context.update(b"\n");
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(context.finish().as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> NodeRuntimeConfig {
        NodeRuntimeConfig::new(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        )
    }

    #[test]
    fn digest_is_stable_across_insertion_order() {
        let first = config(&[
            ("SULION_DB_URL", "postgres://a"),
            ("SULION_RETRIEVAL_TOKEN", "secret"),
        ]);
        let second = config(&[
            ("SULION_RETRIEVAL_TOKEN", "secret"),
            ("SULION_DB_URL", "postgres://a"),
        ]);
        assert_eq!(first.digest(), second.digest());
    }

    #[test]
    fn digest_changes_when_a_credential_rotates() {
        let before = config(&[("SULION_RETRIEVAL_TOKEN", "secret")]);
        let after = config(&[("SULION_RETRIEVAL_TOKEN", "rotated")]);
        assert_ne!(before.digest(), after.digest());
    }

    #[test]
    fn payload_round_trips_without_changing_the_digest() {
        let original = config(&[
            ("SULION_DB_URL", "postgres://sulion@192.168.66.3/sulion"),
            ("SULION_RETRIEVAL_TOKEN", "token"),
        ]);
        let restored = NodeRuntimeConfig::accept(original.payload()).expect("accept payload");
        assert_eq!(original, restored);
    }

    #[test]
    fn key_names_do_not_expose_values() {
        assert_eq!(
            config(&[("SULION_RETRIEVAL_TOKEN", "secret")]).key_names(),
            vec!["SULION_RETRIEVAL_TOKEN"]
        );
    }

    #[test]
    fn rendered_values_are_quoted_and_carry_the_digest() {
        let delivered = config(&[
            ("SULION_DB_URL", "postgres://sulion:p@ss w#rd$1@host/sulion"),
            ("SULION_RETRIEVAL_TOKEN", "token"),
        ]);
        let rendered = delivered.render_env_file();
        assert!(rendered.contains("SULION_DB_URL='postgres://sulion:p@ss w#rd$1@host/sulion'\n"));
        assert!(rendered.contains("SULION_RETRIEVAL_TOKEN='token'\n"));
        assert!(rendered.contains(&format!("{CONFIG_DIGEST_ENV}='{}'\n", delivered.digest())));
    }

    #[test]
    fn a_key_outside_the_allowlist_is_refused() {
        // The delivered file feeds Compose interpolation and a root systemd
        // unit, so an unexpected key would let the peer choose the image, the
        // bind-mount sources, or the privilege-drop identity.
        let hostile = NodeConfigPayload {
            signature: None,
            digest: "irrelevant".into(),
            values: [
                ("SULION_DB_URL".to_string(), "postgres://a".to_string()),
                (
                    "SULION_IMAGE_REGISTRY".to_string(),
                    "ghcr.io/attacker".to_string(),
                ),
            ]
            .into_iter()
            .collect(),
        };
        let error = NodeRuntimeConfig::accept(hostile).expect_err("must refuse");
        assert!(error.to_string().contains("SULION_IMAGE_REGISTRY"));
    }

    #[test]
    fn a_payload_whose_digest_does_not_match_its_values_is_refused() {
        let tampered = NodeConfigPayload {
            signature: None,
            digest: "not-the-real-digest".into(),
            values: [("SULION_DB_URL".to_string(), "postgres://a".to_string())]
                .into_iter()
                .collect(),
        };
        assert!(NodeRuntimeConfig::accept(tampered).is_err());
    }

    #[test]
    fn a_delivered_value_that_breaks_the_env_file_is_refused() {
        let mut payload = config(&[("SULION_DB_URL", "postgres://a")]).payload();
        payload
            .values
            .insert("SULION_RETRIEVAL_TOKEN".into(), "quote'injection".into());
        assert!(NodeRuntimeConfig::accept(payload).is_err());
    }

    #[test]
    fn values_that_quoting_cannot_carry_are_refused() {
        assert!(is_env_file_safe("ordinary-password"));
        assert!(is_env_file_safe("with spaces and $ # symbols"));
        assert!(!is_env_file_safe("has'quote"));
        assert!(!is_env_file_safe("has\nnewline"));
    }

    #[test]
    fn writing_is_idempotent_and_reports_only_real_changes() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("nested").join("delivered.env");

        let delivered = config(&[("SULION_RETRIEVAL_TOKEN", "token")]);
        assert!(delivered.write_delivered(&path).expect("first write"));
        assert!(!delivered.write_delivered(&path).expect("second write"));

        let rotated = config(&[("SULION_RETRIEVAL_TOKEN", "rotated")]);
        assert!(rotated.write_delivered(&path).expect("rotated write"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            rotated.render_env_file()
        );
    }

    #[test]
    fn delivered_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("delivered.env");
        config(&[("SULION_RETRIEVAL_TOKEN", "secret")])
            .write_delivered(&path)
            .expect("write");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "delivered credentials must stay root-only");
    }
}
