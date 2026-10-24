//! The GameStream pairing handshake, including Apollo's OTP variant.
//! Port of `NvPairingManager::pair`.

use std::sync::Arc;
use std::time::Duration;

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use aes::Aes128;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::sha2::{Digest, Sha256};
use rsa::signature::Verifier;
use rsa::RsaPublicKey;
use rustls_pki_types::CertificateDer;
use sha1::Sha1;

use super::http::{HttpError, NvHttp, REQUEST_TIMEOUT};
use super::identity::{Identity, certificate_signature};
use super::xml;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairOutcome {
    Paired(CertificateDer<'static>),
    PinWrong,
    Failed,
    AlreadyInProgress,
}

#[derive(Clone, Copy)]
enum Hash {
    Sha256,
    Sha1,
}

impl Hash {
    fn digest(self, data: &[u8]) -> Vec<u8> {
        match self {
            Hash::Sha256 => Sha256::digest(data).to_vec(),
            Hash::Sha1 => Sha1::digest(data).to_vec(),
        }
    }
    fn len(self) -> usize {
        match self {
            Hash::Sha256 => 32,
            Hash::Sha1 => 20,
        }
    }
}

fn aes_ecb(data: &[u8], key: &[u8], encrypt: bool) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let mut out = data.to_vec();
    for chunk in out.as_chunks_mut::<16>().0 {
        let block = GenericArray::from_mut_slice(chunk);
        if encrypt {
            cipher.encrypt_block(block);
        } else {
            cipher.decrypt_block(block);
        }
    }
    out
}

fn random_bytes(n: usize) -> Vec<u8> {
    (0..n).map(|_| rand::random::<u8>()).collect()
}

/// Verifies `signature` over `data` with the public key of a DER certificate.
fn verify_signature(data: &[u8], signature: &[u8], cert_der: &[u8]) -> bool {
    let Ok((_, cert)) = x509_parser::parse_x509_certificate(cert_der) else { return false };
    let spki = cert.public_key().raw;
    let Ok(pub_key) = RsaPublicKey::from_public_key_der(spki) else { return false };
    let Ok(sig) = Signature::try_from(signature) else { return false };
    VerifyingKey::<Sha256>::new(pub_key).verify(data, &sig).is_ok()
}

pub fn device_name(is_gfe: bool) -> String {
    if is_gfe {
        // GFE expects the legacy name
        "roth".to_string()
    } else {
        let name = gethostname::gethostname().to_string_lossy().to_string();
        if name.is_empty() { "Moonlight".to_string() } else { name }
    }
}

fn device_name_param(is_gfe: bool) -> String {
    let name = device_name(is_gfe);
    format!("devicename={}", percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC))
}

fn major_version(app_version: &str) -> i32 {
    app_version.split('.').next().and_then(|v| v.trim().parse().ok()).unwrap_or(7)
}

/// Runs the pairing handshake. `otp_passphrase` selects Apollo's OTP flow,
/// where `pin` is the host-generated PIN.
pub fn pair(
    http: &mut NvHttp,
    identity: &Arc<Identity>,
    app_version: &str,
    is_gfe: bool,
    pin: &str,
    otp_passphrase: Option<&str>,
) -> Result<PairOutcome, HttpError> {
    let hash = if major_version(app_version) >= 7 { Hash::Sha256 } else { Hash::Sha1 };
    log::info!("Pairing with server generation {}", major_version(app_version));

    let salt = random_bytes(16);
    let mut salted_pin = salt.clone();
    salted_pin.extend_from_slice(pin.as_bytes());
    let mut aes_key = hash.digest(&salted_pin);
    aes_key.truncate(16);

    let dev = device_name_param(is_gfe);
    let unpair = |http: &NvHttp| {
        let _ = http.get_string(false, "unpair", "", REQUEST_TIMEOUT);
    };

    let mut get_cert_args = format!(
        "{dev}&updateState=1&phrase=getservercert&salt={}&clientcert={}",
        hex::encode(&salt),
        hex::encode(identity.cert_pem.as_bytes())
    );
    if let Some(passphrase) = otp_passphrase {
        // Apollo OTP: prove knowledge of the PIN and passphrase up front
        let otp = Sha256::digest(format!("{pin}{}{passphrase}", hex::encode(&salt)).as_bytes());
        get_cert_args.push_str(&format!("&otpauth={}", hex::encode_upper(otp)));
    }

    // This request blocks until the user enters the PIN on the host (no timeout),
    // except in the OTP flow where the host answers immediately.
    let timeout = if otp_passphrase.is_some() { REQUEST_TIMEOUT } else { Duration::ZERO };
    let get_cert = http.get_string(false, "pair", &get_cert_args, timeout)?;
    xml::verify_status(&get_cert)?;
    if xml::get_string(&get_cert, "paired").as_deref() != Some("1") {
        log::error!("Failed pairing at stage #1");
        return Ok(PairOutcome::Failed);
    }

    let server_cert_pem = xml::get_hex(&get_cert, "plaincert");
    if server_cert_pem.is_empty() {
        log::error!("Server likely already pairing");
        unpair(http);
        return Ok(PairOutcome::AlreadyInProgress);
    }
    let server_cert_pem_str = String::from_utf8_lossy(&server_cert_pem).into_owned();
    let Some(server_cert) = super::identity::pem_to_der(&server_cert_pem_str) else {
        log::error!("Failed to parse plaincert");
        unpair(http);
        return Ok(PairOutcome::Failed);
    };

    // Pin this cert for TLS until pairing completes
    http.set_server_cert(Some(server_cert.clone()));

    let challenge = random_bytes(16);
    let encrypted_challenge = aes_ecb(&challenge, &aes_key, true);
    let challenge_xml = http.get_string(false, "pair", &format!("{dev}&updateState=1&clientchallenge={}", hex::encode(&encrypted_challenge)), REQUEST_TIMEOUT)?;
    xml::verify_status(&challenge_xml)?;
    if xml::get_string(&challenge_xml, "paired").as_deref() != Some("1") {
        log::error!("Failed pairing at stage #2");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }

    let challenge_response = aes_ecb(&xml::get_hex(&challenge_xml, "challengeresponse"), &aes_key, false);
    if challenge_response.len() < hash.len() + 16 {
        log::error!("Invalid challengeresponse at stage #2");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }

    let client_secret = random_bytes(16);
    let server_response = challenge_response[..hash.len()].to_vec();
    let mut response = Vec::new();
    response.extend_from_slice(&challenge_response[hash.len()..hash.len() + 16]);
    response.extend_from_slice(identity.cert_signature());
    response.extend_from_slice(&client_secret);

    let mut padded_hash = hash.digest(&response);
    padded_hash.resize(32, 0);
    let encrypted_response_hash = aes_ecb(&padded_hash, &aes_key, true);
    let resp_xml = http.get_string(false, "pair", &format!("{dev}&updateState=1&serverchallengeresp={}", hex::encode(&encrypted_response_hash)), REQUEST_TIMEOUT)?;
    xml::verify_status(&resp_xml)?;
    if xml::get_string(&resp_xml, "paired").as_deref() != Some("1") {
        log::error!("Failed pairing at stage #3");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }

    let pairing_secret = xml::get_hex(&resp_xml, "pairingsecret");
    if pairing_secret.len() <= 16 {
        log::error!("Invalid pairingsecret at stage #3");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }
    let server_secret = &pairing_secret[..16];
    let server_signature = &pairing_secret[16..];

    if !verify_signature(server_secret, server_signature, server_cert.as_ref()) {
        log::error!("MITM detected");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }

    let mut expected = Vec::new();
    expected.extend_from_slice(&challenge);
    expected.extend_from_slice(&certificate_signature(server_cert.as_ref()).unwrap_or_default());
    expected.extend_from_slice(server_secret);
    if hash.digest(&expected) != server_response {
        log::error!("Incorrect PIN");
        unpair(http);
        return Ok(PairOutcome::PinWrong);
    }

    let mut client_pairing_secret = client_secret.clone();
    client_pairing_secret.extend_from_slice(&identity.sign(&client_secret));
    let secret_xml = http.get_string(false, "pair", &format!("{dev}&updateState=1&clientpairingsecret={}", hex::encode(&client_pairing_secret)), REQUEST_TIMEOUT)?;
    xml::verify_status(&secret_xml)?;
    if xml::get_string(&secret_xml, "paired").as_deref() != Some("1") {
        log::error!("Failed pairing at stage #4");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }

    let challenge_xml = http.get_string(true, "pair", &format!("{dev}&updateState=1&phrase=pairchallenge"), REQUEST_TIMEOUT)?;
    xml::verify_status(&challenge_xml)?;
    if xml::get_string(&challenge_xml, "paired").as_deref() != Some("1") {
        log::error!("Failed pairing at stage #5");
        unpair(http);
        return Ok(PairOutcome::Failed);
    }

    Ok(PairOutcome::Paired(server_cert))
}

pub fn generate_pin() -> String {
    format!("{:04}", rand::random::<u32>() % 10000)
}
