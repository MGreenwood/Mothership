//! GhostID provider identity for Mothership.
//!
//! Every GhostID login challenge is signed with the provider's Ed25519 key. A wallet
//! trusts that signature only when the exact provider hostname publishes the matching
//! `ghostid-provider-pk=` TXT record, so the record is checked live when a challenge is
//! issued and again before a login is accepted.

use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};
use tracing::info;

const PROVIDER_TXT_PREFIX: &str = "ghostid-provider-pk=";
const PROVIDER_MESSAGE_TAG: &str = "GhostID Provider Challenge v1";
const PRIVATE_KEY_FILE_ENV: &str = "PROVIDER_PRIVATE_KEY_FILE";

/// The fields of a login challenge that the provider signs, in signing order.
#[derive(Clone, Debug, PartialEq)]
pub struct SignedChallenge {
    pub challenge_id: String,
    pub domain: String,
    pub action: String,
    pub nonce: String,
    pub issued_at: i64,
    pub expires_at: i64,
    pub permissions: Vec<String>,
    pub duration_days: u32,
    pub tier: String,
}

struct ProviderKey {
    key_pair: Ed25519KeyPair,
    public_key_b64: String,
}

lazy_static::lazy_static! {
    static ref PROVIDER_KEY: Option<ProviderKey> = load_from_file();
}

/// Loads the provider key from the PEM file named by `PROVIDER_PRIVATE_KEY_FILE`.
/// The file must stay outside any public directory; its contents are never logged.
fn load_from_file() -> Option<ProviderKey> {
    let path = std::env::var(PRIVATE_KEY_FILE_ENV).ok()?;
    let pem = std::fs::read_to_string(&path).ok()?;
    let key_pair = parse_pem_pkcs8(&pem).ok()?;
    let public_key_b64 = base64::engine::general_purpose::STANDARD.encode(key_pair.public_key().as_ref());
    info!("GhostID provider key loaded from {}", PRIVATE_KEY_FILE_ENV);
    Some(ProviderKey { key_pair, public_key_b64 })
}

/// Parses an Ed25519 PKCS#8 PEM private key, as written by GhostID's onboarding tool
/// (PKCS#8 v1 48-byte seed-only DER) or `ring` (PKCS#8 v2 with public key).
pub fn parse_pem_pkcs8(pem: &str) -> Result<Ed25519KeyPair, String> {
    if !pem.contains("BEGIN PRIVATE KEY") {
        return Err("expected a PKCS#8 PEM private key".to_string());
    }
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|_| "private key is not valid base64".to_string())?;
    Ed25519KeyPair::from_pkcs8(&der)
        .or_else(|_| Ed25519KeyPair::from_pkcs8_maybe_unchecked(&der))
        .map_err(|_| "private key is not an Ed25519 PKCS#8 key".to_string())
}

/// True when a provider key is configured. Without it, GhostID login is unavailable.
pub fn is_configured() -> bool {
    PROVIDER_KEY.is_some()
}

/// Base64 public key of the loaded provider key, which must match the DNS record.
pub fn public_key_b64() -> Option<String> {
    PROVIDER_KEY.as_ref().map(|key| key.public_key_b64.clone())
}

/// Canonical message the provider signs. Must match `providerMessage` in the GhostID
/// auth-server field for field.
pub fn provider_message(challenge: &SignedChallenge) -> String {
    serde_json::json!([
        PROVIDER_MESSAGE_TAG,
        challenge.challenge_id,
        challenge.domain,
        challenge.action,
        challenge.nonce,
        challenge.issued_at,
        challenge.expires_at,
        challenge.permissions,
        challenge.duration_days,
        challenge.tier,
    ])
    .to_string()
}

/// Signs a challenge with the provider key. Returns the signature as base64,
/// or `None` when no provider key is configured.
pub fn sign_challenge(challenge: &SignedChallenge) -> Option<String> {
    let key = PROVIDER_KEY.as_ref()?;
    let signature = key.key_pair.sign(provider_message(challenge).as_bytes());
    Some(base64::engine::general_purpose::STANDARD.encode(signature.as_ref()))
}

/// True only when exactly one `ghostid-provider-pk=` record exists and it carries
/// `expected_public_key_b64`. Other TXT records are ignored.
pub fn txt_records_publish_key(records: &[String], expected_public_key_b64: &str) -> bool {
    let mut matches = records.iter().filter_map(|record| record.strip_prefix(PROVIDER_TXT_PREFIX));
    match (matches.next(), matches.next()) {
        (Some(value), None) => value == expected_public_key_b64,
        _ => false,
    }
}

/// Looks up the TXT records at `domain` and checks that they publish this server's
/// provider key. Fails closed on any lookup error.
pub async fn verify_published_key(domain: &str) -> Result<(), String> {
    let public_key = public_key_b64().ok_or_else(|| "GhostID provider key is not configured".to_string())?;
    let (sys_cfg, mut opts) = hickory_resolver::system_conf::read_system_conf()
        .unwrap_or_else(|_| (hickory_resolver::config::ResolverConfig::cloudflare(), hickory_resolver::config::ResolverOpts::default()));
    opts.timeout = std::time::Duration::from_secs(3);
    opts.attempts = 1;
    let mut cfg = hickory_resolver::config::ResolverConfig::new();
    for ns in sys_cfg.name_servers() {
        // Skip Windows disconnected-adapter site-local IPv6 placeholders (fec0::/10)
        if let std::net::IpAddr::V6(v6) = ns.socket_addr.ip() {
            if (v6.segments()[0] & 0xffc0) == 0xfec0 {
                continue;
            }
        }
        cfg.add_name_server(ns.clone());
    }
    if cfg.name_servers().is_empty() {
        cfg = hickory_resolver::config::ResolverConfig::cloudflare();
    }
    let resolver = hickory_resolver::TokioAsyncResolver::tokio(cfg, opts);
    let lookup = resolver
        .txt_lookup(domain)
        .await
        .map_err(|e| format!("TXT lookup for {domain} failed: {e}"))?;
    // A TXT record can be split into chunks; join them before parsing.
    let records: Vec<String> = lookup
        .iter()
        .map(|txt| txt.txt_data().iter().map(|chunk| String::from_utf8_lossy(chunk)).collect::<String>())
        .collect();
    if txt_records_publish_key(&records, &public_key) {
        Ok(())
    } else {
        Err(format!("TXT record at {domain} does not publish this provider key"))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::UnparsedPublicKey;

    fn sample_challenge() -> SignedChallenge {
        SignedChallenge {
            challenge_id: "chal-1".to_string(),
            domain: "mothership.rest".to_string(),
            action: "login".to_string(),
            nonce: "nonce-1".to_string(),
            issued_at: 1_700_000_000,
            expires_at: 1_700_000_180,
            permissions: vec!["login".to_string()],
            duration_days: 30,
            tier: "basic".to_string(),
        }
    }


    fn generate_pem() -> (String, Vec<u8>) {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).expect("keygen");
        let body = base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref());
        let pem = format!("-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", body);
        (pem, pkcs8.as_ref().to_vec())
    }

    #[test]
    fn parses_pem_and_signs_provider_message() {
        let (pem, pkcs8) = generate_pem();
        let key_pair = parse_pem_pkcs8(&pem).expect("parse PEM");
        let expected = Ed25519KeyPair::from_pkcs8(&pkcs8).expect("parse DER");
        assert_eq!(key_pair.public_key().as_ref(), expected.public_key().as_ref());

        let challenge = sample_challenge();
        let signature = key_pair.sign(provider_message(&challenge).as_bytes());
        assert!(UnparsedPublicKey::new(&ring::signature::ED25519, key_pair.public_key().as_ref())
            .verify(provider_message(&challenge).as_bytes(), signature.as_ref())
            .is_ok());
    }

    #[test]
    fn rejects_non_pkcs8_input() {
        assert!(parse_pem_pkcs8("not a key").is_err());
        assert!(parse_pem_pkcs8("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----").is_err());
    }

    #[test]
    fn provider_message_matches_auth_server_format() {
        assert_eq!(
            provider_message(&sample_challenge()),
            r#"["GhostID Provider Challenge v1","chal-1","mothership.rest","login","nonce-1",1700000000,1700000180,["login"],30,"basic"]"#
        );
    }

    #[test]
    fn txt_record_must_publish_exactly_this_key() {
        let key = "AAAA";
        let good = format!("{PROVIDER_TXT_PREFIX}{key}");
        assert!(txt_records_publish_key(&[good.clone()], key));
        assert!(txt_records_publish_key(&["v=spf1 -all".to_string(), good.clone()], key));
        assert!(!txt_records_publish_key(&[], key));
        assert!(!txt_records_publish_key(&[format!("{PROVIDER_TXT_PREFIX}BBBB")], key));
        assert!(!txt_records_publish_key(&[good.clone(), good.clone()], key));
        assert!(!txt_records_publish_key(&[format!("{PROVIDER_TXT_PREFIX}{key}X")], key));
    }
}
