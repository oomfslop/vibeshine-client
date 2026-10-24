//! The client identity: a self-signed RSA certificate, its private key and a
//! random unique ID. Mirrors moonlight-qt's `IdentityManager`.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer};
use rsa::RsaPrivateKey;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    unique_id: String,
    certificate_pem: String,
    private_key_pem: String,
}

pub struct Identity {
    pub unique_id: String,
    pub cert_pem: String,
    pub key_pem: String,
    cert_der: CertificateDer<'static>,
    key_der: PrivateKeyDer<'static>,
    signing_key: SigningKey<Sha256>,
    /// The signatureValue of our certificate, used in the pairing handshake.
    cert_signature: Vec<u8>,
}

impl Identity {
    /// Loads the identity from `dir/identity.json`, creating (and persisting)
    /// a new one when missing or unreadable.
    pub fn load_or_create(dir: &Path) -> Result<Arc<Identity>> {
        let path = dir.join("identity.json");
        if let Ok(bytes) = fs::read(&path) {
            match serde_json::from_slice::<StoredIdentity>(&bytes)
                .map_err(anyhow::Error::from)
                .and_then(Identity::from_stored)
            {
                Ok(identity) => return Ok(Arc::new(identity)),
                Err(e) => log::warn!("Stored identity unreadable, generating a new one: {e}"),
            }
        }

        log::info!("Generating new client identity");
        let stored = Identity::generate()?;
        fs::create_dir_all(dir)?;
        fs::write(&path, serde_json::to_vec_pretty(&stored)?)?;
        Ok(Arc::new(Identity::from_stored(stored)?))
    }

    fn generate() -> Result<StoredIdentity> {
        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048).context("RSA key generation failed")?;
        let key_pem = key.to_pkcs8_pem(LineEnding::LF)?.to_string();

        let key_pair = rcgen::KeyPair::from_pem(&key_pem).context("rcgen rejected RSA key")?;
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "NVIDIA GameStream Client");
        let now = time::OffsetDateTime::now_utc();
        params.not_before = now;
        params.not_after = now + time::Duration::days(365 * 20);
        params.serial_number = Some(rcgen::SerialNumber::from(vec![0u8]));
        let cert = params.self_signed(&key_pair)?;

        let unique_id: u64 = rand::random();
        Ok(StoredIdentity {
            unique_id: format!("{unique_id:x}"),
            certificate_pem: cert.pem(),
            private_key_pem: key_pem,
        })
    }

    fn from_stored(stored: StoredIdentity) -> Result<Identity> {
        let cert_der = rustls_pemfile::certs(&mut stored.certificate_pem.as_bytes())
            .next()
            .ok_or_else(|| anyhow!("no certificate in PEM"))??;
        let key_der = rustls_pemfile::private_key(&mut stored.private_key_pem.as_bytes())?
            .ok_or_else(|| anyhow!("no private key in PEM"))?;
        let rsa_key = RsaPrivateKey::from_pkcs8_pem(&stored.private_key_pem)
            .or_else(|_| {
                use rsa::pkcs1::DecodeRsaPrivateKey;
                RsaPrivateKey::from_pkcs1_pem(&stored.private_key_pem)
            })
            .context("private key is not RSA")?;
        let cert_signature = certificate_signature(&cert_der)?;
        if stored.unique_id.is_empty() {
            return Err(anyhow!("empty unique id"));
        }
        Ok(Identity {
            unique_id: stored.unique_id,
            cert_pem: stored.certificate_pem,
            key_pem: stored.private_key_pem,
            cert_der,
            key_der,
            signing_key: SigningKey::<Sha256>::new(rsa_key),
            cert_signature,
        })
    }

    pub fn cert_der(&self) -> &CertificateDer<'static> {
        &self.cert_der
    }

    pub fn key_der(&self) -> PrivateKeyDer<'static> {
        self.key_der.clone_key()
    }

    pub fn cert_signature(&self) -> &[u8] {
        &self.cert_signature
    }

    /// RSA PKCS#1 v1.5 SHA-256 signature, as `EVP_DigestSign` produces.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signing_key.sign(message).to_vec()
    }
}

/// Extracts the signatureValue bit string from a DER certificate.
pub fn certificate_signature(der: &[u8]) -> Result<Vec<u8>> {
    let (_, cert) = x509_parser::parse_x509_certificate(der)
        .map_err(|e| anyhow!("certificate parse error: {e}"))?;
    Ok(cert.signature_value.data.to_vec())
}

/// Parses the first PEM certificate into DER.
pub fn pem_to_der(pem: &str) -> Option<CertificateDer<'static>> {
    rustls_pemfile::certs(&mut pem.as_bytes()).next()?.ok()
}

/// Encodes a DER certificate as PEM.
pub fn der_to_pem(der: &[u8]) -> String {
    use std::fmt::Write;
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut b64 = String::with_capacity(der.len() * 4 / 3 + 4);
    for chunk in der.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        b64.push(TABLE[(n >> 18) as usize & 63] as char);
        b64.push(TABLE[(n >> 12) as usize & 63] as char);
        b64.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        b64.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    let mut out = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in b64.as_bytes().chunks(64) {
        let _ = writeln!(out, "{}", std::str::from_utf8(line).unwrap());
    }
    out.push_str("-----END CERTIFICATE-----\n");
    out
}
