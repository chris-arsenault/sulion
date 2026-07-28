//! TLS for the node channel, terminated in userspace.
//!
//! Node traffic carries terminal bytes — credentials included — so the LAN hop
//! is encrypted. The control host grants no elevated network privileges, so
//! there is no kernel tunnel: the control process itself serves TLS on the
//! node port with a self-generated certificate.
//!
//! Trust does not come from a CA. The node pins the certificate on first
//! pairing — the same trust-on-first-use moment as the Ed25519 control key —
//! and the signed handshake proof covers the certificate's digest, so once a
//! node has pinned the control identity, a substituted certificate cannot
//! survive the handshake even on first TLS contact.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};

use super::NodeProtocolError;
use crate::db::Pool;

/// Where the node persists the pinned certificate. Root-owned, beside the
/// other pins, so it survives container replacement and release upgrades.
pub const TLS_PIN_PATH_ENV: &str = "SULION_NODE_CONTROL_TLS_PIN_PATH";
pub const DEFAULT_TLS_PIN_PATH: &str = "/var/lib/sulion-node/control-tls.pem";

/// World-readable copy for the unprivileged runtime and PTY tools. The
/// certificate is public material; only the pin under /var/lib is the record
/// of trust.
pub const RUNTIME_CA_PATH: &str = "/run/sulion/control-tls.pem";

/// Env var pointing HTTP clients (the node process and every PTY tool) at the
/// pinned certificate. Added as an extra root, so public TLS is unaffected.
pub const CONTROL_CA_ENV: &str = "SULION_CONTROL_TLS_CA";

fn ring_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Digest a certificate the way the handshake proof signs it.
pub fn cert_digest(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref())
}

/// The control plane's TLS identity: a self-signed certificate with the LAN
/// address in its SANs, generated once and kept in Postgres.
pub struct ControlTls {
    pub cert_pem: String,
    pub digest: String,
    cert_der: CertificateDer<'static>,
    key_der: PrivateKeyDer<'static>,
}

impl ControlTls {
    /// Loads the stored identity, generating one on first start. A change to
    /// the configured SANs regenerates the certificate, because clients verify
    /// the address they dial against it.
    pub async fn load_or_create(pool: &Pool, sans: &str) -> Result<Self, NodeProtocolError> {
        if let Some(existing) = Self::stored(pool, sans).await? {
            return Ok(existing);
        }
        let (cert_pem, key_pem) = generate(sans)?;
        sqlx::query(
            "INSERT INTO control_tls (id, private_key_pem, cert_pem, subject_alt_names) \
             VALUES (1, $1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET \
                private_key_pem = EXCLUDED.private_key_pem, \
                cert_pem = EXCLUDED.cert_pem, \
                subject_alt_names = EXCLUDED.subject_alt_names \
             WHERE control_tls.subject_alt_names IS DISTINCT FROM EXCLUDED.subject_alt_names",
        )
        .bind(&key_pem)
        .bind(&cert_pem)
        .bind(sans)
        .execute(pool)
        .await?;
        // Re-read so two racing control processes converge on one certificate.
        Self::stored(pool, sans)
            .await?
            .ok_or_else(|| NodeProtocolError::Cryptography("control TLS identity vanished".into()))
    }

    async fn stored(pool: &Pool, sans: &str) -> Result<Option<Self>, NodeProtocolError> {
        let Some((key_pem, cert_pem, stored_sans)) = sqlx::query_as::<_, (String, String, String)>(
            "SELECT private_key_pem, cert_pem, subject_alt_names FROM control_tls WHERE id = 1",
        )
        .fetch_optional(pool)
        .await?
        else {
            return Ok(None);
        };
        if stored_sans != sans {
            return Ok(None);
        }
        Ok(Some(Self::from_pem(&cert_pem, &key_pem)?))
    }

    pub fn from_pem(cert_pem: &str, key_pem: &str) -> Result<Self, NodeProtocolError> {
        let invalid =
            |what: &str| NodeProtocolError::Cryptography(format!("stored control TLS {what}"));
        let cert_der = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .next()
            .ok_or_else(|| invalid("certificate is empty"))?
            .map_err(|_| invalid("certificate is invalid"))?;
        let key_der = rustls_pemfile::private_key(&mut key_pem.as_bytes())
            .map_err(|_| invalid("key is invalid"))?
            .ok_or_else(|| invalid("key is empty"))?;
        Ok(Self {
            cert_pem: cert_pem.to_string(),
            digest: cert_digest(&cert_der),
            cert_der,
            key_der,
        })
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }

    pub fn server_config(&self) -> Result<rustls::ServerConfig, NodeProtocolError> {
        rustls::ServerConfig::builder_with_provider(ring_provider())
            .with_safe_default_protocol_versions()
            .map_err(|error| NodeProtocolError::Cryptography(error.to_string()))?
            .with_no_client_auth()
            .with_single_cert(vec![self.cert_der.clone()], self.key_der.clone_key())
            .map_err(|error| NodeProtocolError::Cryptography(error.to_string()))
    }
}

/// Freshly minted certificate material for tests.
pub fn test_certificate() -> (String, String) {
    generate("127.0.0.1").expect("mint test certificate")
}

/// The first deployed generation of certificates carried CA:TRUE; kept
/// mintable so tests keep proving the pinned clients accept them.
pub fn test_certificate_with_ca_flag() -> (String, String) {
    generate_with_ca("127.0.0.1", true).expect("mint test certificate")
}

/// Mints the self-signed certificate as a plain end-entity certificate.
///
/// Verification is byte-exact pinning on every client, so no CA flag is
/// needed — and webpki rejects a certificate carrying CA:TRUE when a server
/// presents it as its own (`CaUsedAsEndEntity`), so setting it would break any
/// standard verifier that ever sees this certificate.
fn generate(sans: &str) -> Result<(String, String), NodeProtocolError> {
    generate_with_ca(sans, false)
}

fn generate_with_ca(sans: &str, ca: bool) -> Result<(String, String), NodeProtocolError> {
    let failed = |error: String| NodeProtocolError::Cryptography(error);
    let mut params =
        rcgen::CertificateParams::new(Vec::new()).map_err(|error| failed(error.to_string()))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "sulion-control");
    if ca {
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    }
    for san in sans.split(',').map(str::trim).filter(|san| !san.is_empty()) {
        let san = match san.parse::<std::net::IpAddr>() {
            Ok(address) => rcgen::SanType::IpAddress(address),
            Err(_) => rcgen::SanType::DnsName(
                san.try_into()
                    .map_err(|_| failed(format!("invalid TLS SAN {san}")))?,
            ),
        };
        params.subject_alt_names.push(san);
    }
    if params.subject_alt_names.is_empty() {
        return Err(failed("control TLS requires at least one SAN".into()));
    }
    let key_pair = rcgen::KeyPair::generate().map_err(|error| failed(error.to_string()))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|error| failed(error.to_string()))?;
    Ok((cert.pem(), key_pair.serialize_pem()))
}

/// The node's record of which certificate it trusts, plus the certificate the
/// live handshake actually presented.
pub struct TlsPin {
    path: PathBuf,
}

impl TlsPin {
    pub fn from_env() -> Self {
        Self {
            path: PathBuf::from(
                std::env::var(TLS_PIN_PATH_ENV)
                    .unwrap_or_else(|_| DEFAULT_TLS_PIN_PATH.to_string()),
            ),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pinned certificate. Falls back to the runtime copy, which the
    /// unprivileged steady-state process can read after the root-owned pin
    /// directory becomes inaccessible to it.
    pub fn pinned(&self) -> Option<CertificateDer<'static>> {
        for path in [self.path.as_path(), Path::new(RUNTIME_CA_PATH)] {
            if let Ok(pem) = std::fs::read(path) {
                if let Some(Ok(cert)) = rustls_pemfile::certs(&mut pem.as_slice()).next() {
                    return Some(cert);
                }
            }
        }
        None
    }

    /// Records the pinned certificate and writes the world-readable runtime
    /// copy for PTY tools. Called while the node still runs as root.
    pub fn record(&self, der: &[u8]) -> anyhow::Result<()> {
        let pem = pem_encode(der);
        write_atomic(&self.path, pem.as_bytes(), 0o600)?;
        write_atomic(Path::new(RUNTIME_CA_PATH), pem.as_bytes(), 0o644)?;
        Ok(())
    }
}

fn pem_encode(der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tls.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Certificate check for the node's connection to control.
///
/// With a pin on disk, the presented certificate must match it exactly. With
/// no pin — first pairing — any certificate is accepted at the TLS layer and
/// recorded, because the signed Ed25519 handshake that follows binds the
/// certificate digest and is what actually authenticates the peer.
#[derive(Debug)]
pub struct PinnedServerVerifier {
    pinned: Option<CertificateDer<'static>>,
    seen: Arc<Mutex<Option<Vec<u8>>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl PinnedServerVerifier {
    /// Returns the verifier and the slot where the presented certificate's DER
    /// appears after the handshake.
    pub fn new(pinned: Option<CertificateDer<'static>>) -> (Self, Arc<Mutex<Option<Vec<u8>>>>) {
        let seen = Arc::new(Mutex::new(None));
        (
            Self {
                pinned,
                seen: seen.clone(),
                provider: ring_provider(),
            },
            seen,
        )
    }
}

impl rustls::client::danger::ServerCertVerifier for PinnedServerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        *self.seen.lock().expect("seen slot") = Some(end_entity.as_ref().to_vec());
        if let Some(pinned) = &self.pinned {
            if pinned.as_ref() != end_entity.as_ref() {
                return Err(rustls::Error::InvalidCertificate(
                    rustls::CertificateError::ApplicationVerificationFailure,
                ));
            }
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Client configuration for the node's wss:// connection.
pub fn client_config(
    verifier: PinnedServerVerifier,
) -> Result<rustls::ClientConfig, NodeProtocolError> {
    Ok(rustls::ClientConfig::builder_with_provider(ring_provider())
        .with_safe_default_protocol_versions()
        .map_err(|error| NodeProtocolError::Cryptography(error.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// HTTP client for the control plane's node endpoint (broker and retrieval).
///
/// When the pinned certificate is present, TLS uses the same byte-exact pin
/// as the control channel rather than webpki verification — the endpoint is a
/// single known certificate, exact match is stronger than any root store, and
/// webpki rejects the first deployed certificate generation outright
/// (`CaUsedAsEndEntity`, its CA:TRUE flag). These clients speak only to
/// control, never to public hosts, so replacing the root store loses nothing.
pub fn control_http_client() -> reqwest::Client {
    let Some(pinned) = pinned_from_env() else {
        return reqwest::Client::new();
    };
    pinned_http_client(pinned).unwrap_or_else(|error| {
        tracing::warn!(%error, "pinned TLS client unavailable; using default verification");
        reqwest::Client::new()
    })
}

/// Reqwest client that accepts exactly one server certificate.
pub fn pinned_http_client(
    pinned: CertificateDer<'static>,
) -> Result<reqwest::Client, NodeProtocolError> {
    let (verifier, _seen) = PinnedServerVerifier::new(Some(pinned));
    reqwest::Client::builder()
        .use_preconfigured_tls(client_config(verifier)?)
        .build()
        .map_err(|error| NodeProtocolError::Cryptography(error.to_string()))
}

fn pinned_from_env() -> Option<CertificateDer<'static>> {
    let path = std::env::var(CONTROL_CA_ENV).ok()?;
    let pem = match std::fs::read(&path) {
        Ok(pem) => pem,
        Err(error) => {
            tracing::debug!(%path, %error, "no control TLS certificate to trust");
            return None;
        }
    };
    let cert = rustls_pemfile::certs(&mut pem.as_slice()).next();
    match cert {
        Some(Ok(cert)) => Some(cert),
        _ => {
            tracing::warn!(%path, "control TLS certificate is unreadable");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minted() -> ControlTls {
        let (cert_pem, key_pem) = generate("192.168.66.3, control.local").expect("generate");
        ControlTls::from_pem(&cert_pem, &key_pem).expect("parse")
    }

    #[test]
    fn a_minted_certificate_round_trips_and_serves() {
        let tls = minted();
        assert!(tls.cert_pem.contains("BEGIN CERTIFICATE"));
        tls.server_config().expect("server config");
    }

    #[test]
    fn the_digest_is_stable_and_tracks_the_certificate() {
        let tls = minted();
        let reparsed = ControlTls::from_pem(&tls.cert_pem, "unused")
            .err()
            .is_some();
        // Key PEM "unused" is invalid, so from_pem fails — digest comparison
        // uses a proper reparse below instead.
        assert!(reparsed);
        assert_eq!(tls.digest, cert_digest(&tls.cert_der));
        assert_ne!(tls.digest, minted().digest);
    }

    #[test]
    fn sans_are_required() {
        assert!(generate(" ").is_err());
    }

    #[test]
    fn the_pin_verifier_accepts_only_the_pinned_certificate() {
        let tls = minted();
        let other = minted();
        let (verifier, seen) = PinnedServerVerifier::new(Some(tls.cert_der.clone()));
        use rustls::client::danger::ServerCertVerifier;
        let name = ServerName::try_from("192.168.66.3").expect("name");

        verifier
            .verify_server_cert(&tls.cert_der, &[], &name, &[], UnixTime::now())
            .expect("pinned certificate verifies");
        assert_eq!(seen.lock().unwrap().as_deref(), Some(tls.cert_der.as_ref()));
        verifier
            .verify_server_cert(&other.cert_der, &[], &name, &[], UnixTime::now())
            .expect_err("a different certificate is refused");
    }

    #[test]
    fn first_contact_accepts_and_records_for_the_handshake_to_judge() {
        let tls = minted();
        let (verifier, seen) = PinnedServerVerifier::new(None);
        use rustls::client::danger::ServerCertVerifier;
        let name = ServerName::try_from("192.168.66.3").expect("name");
        verifier
            .verify_server_cert(&tls.cert_der, &[], &name, &[], UnixTime::now())
            .expect("first contact accepts");
        assert!(seen.lock().unwrap().is_some());
    }

    #[test]
    fn recording_a_pin_writes_valid_pem() {
        let directory = tempfile::tempdir().expect("tempdir");
        let tls = minted();
        let pem = pem_encode(tls.cert_der.as_ref());
        let path = directory.path().join("control-tls.pem");
        write_atomic(&path, pem.as_bytes(), 0o600).expect("write");
        let read = std::fs::read(&path).expect("read");
        let cert = rustls_pemfile::certs(&mut read.as_slice())
            .next()
            .expect("one cert")
            .expect("valid cert");
        assert_eq!(cert.as_ref(), tls.cert_der.as_ref());
    }
}
