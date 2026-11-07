//! 智慧树 protocol crypto.
//!
//! Two primitives, both taken from the public student client:
//!
//! 1. **Bootstrap key exchange.** The module's AES key for a gateway (`protocol_key`) is not
//!    negotiated over TLS fields: a JSON `{"module":N}` block is RSA-encrypted with a constant
//!    server public key, sent as `?uid=<base64>`, and the reply `rt.sl` is an RSA *private-key*
//!    operation (type-1 padding) that we undo with the same public key. The recovered block
//!    carries `cKey`, a 16-byte AES-128 key.
//! 2. **Request signing.** Gateway payloads are JSON, AES-128-CBC encrypted with `cKey` and a
//!    fixed IV, PKCS#7 padded, then base64 — the `secretStr` form field.
//!
//! This is protocol cryptography, not credential handling: no user secret passes through here,
//! and the AES key is scoped to one session's gateway calls.

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use rsa::{
    pkcs8::DecodePublicKey, traits::PublicKeyParts, BigUint, Pkcs1v15Encrypt, RsaPublicKey,
};
use serde_json::Value;

use crate::error::ModuleError;

/// Public protocol parameter from the official student client. A server public key, not a
/// credential: it only protects the in-band key exchange.
const BOOTSTRAP_KEY: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCgfZmpLpPEpEFRKBe+ZjWJUjPe+7qg7pGqcfN3j2egJ8H2mrKwaEqZEnPnpi2O3hN8HRyaFozDOp8gwZiYfiIZjWy0Jr/FNAiiKYh5bq0GsEn+ieMmRyJg/+i1rqizhvCXvFdrdGhFTw5EBwTpsGdwe1utdlrvIJUAFWj9Yh4qbQIDAQAB";

/// Fixed IV used by the student client for every signed gateway payload.
const QUERY_IV: &[u8; 16] = b"1g3qqdh4jvbskb9x";

fn protocol_error() -> ModuleError {
    ModuleError::protocol_changed("智慧树返回格式与当前实现不一致，请保留页面提示以便核对")
}

pub fn bootstrap_public_key() -> Result<RsaPublicKey, ModuleError> {
    let der = STANDARD
        .decode(BOOTSTRAP_KEY)
        .map_err(|_| protocol_error())?;
    RsaPublicKey::from_public_key_der(&der).map_err(|_| protocol_error())
}

/// `uid` query value for the key service: RSA-PKCS#1 v1.5 of `{"module":N}`, base64.
pub fn encrypt_module_request(key: &RsaPublicKey, module: u8) -> Result<String, ModuleError> {
    let payload = serde_json::to_vec(&serde_json::json!({ "module": module }))
        .map_err(|_| protocol_error())?;
    let mut rng = rand::rngs::OsRng;
    let encrypted = key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &payload)
        .map_err(|_| protocol_error())?;
    Ok(STANDARD.encode(encrypted))
}

/// Undo the server's private-key operation on `rt.sl` and return the 16-byte AES key.
pub fn recover_protocol_key(key: &RsaPublicKey, signed: &str) -> Result<Vec<u8>, ModuleError> {
    let bytes = STANDARD.decode(signed).map_err(|_| protocol_error())?;
    if bytes.len() != key.size() {
        return Err(protocol_error());
    }
    let ciphertext = BigUint::from_bytes_be(&bytes);
    if &ciphertext >= key.n() {
        return Err(protocol_error());
    }
    let message = ciphertext.modpow(key.e(), key.n()).to_bytes_be();
    let mut padded = vec![0; key.size().saturating_sub(message.len())];
    padded.extend(message);
    // Type-1 padding: 00 01 FF.. 00 <json>
    if !padded.starts_with(&[0, 1]) {
        return Err(protocol_error());
    }
    let end = padded[2..]
        .iter()
        .position(|byte| *byte == 0)
        .map(|index| index + 2)
        .ok_or_else(protocol_error)?;
    if end < 10 || !padded[2..end].iter().all(|byte| *byte == 0xFF) {
        return Err(protocol_error());
    }
    let reply: Value = serde_json::from_slice(&padded[end + 1..]).map_err(|_| protocol_error())?;
    let key = reply["cKey"]
        .as_str()
        .ok_or_else(protocol_error)?
        .as_bytes()
        .to_vec();
    if key.len() != 16 {
        return Err(protocol_error());
    }
    Ok(key)
}

/// `secretStr`: JSON → AES-128-CBC(cKey, fixed IV, PKCS#7) → base64.
pub fn encode_query(payload: &Value, key: &[u8]) -> Result<String, ModuleError> {
    let bytes = serde_json::to_vec(payload).map_err(|_| protocol_error())?;
    let cipher =
        cbc::Encryptor::<aes::Aes128>::new_from_slices(key, QUERY_IV).map_err(|_| protocol_error())?;
    Ok(STANDARD.encode(cipher.encrypt_padded_vec_mut::<Pkcs7>(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::traits::PrivateKeyParts;
    use rsa::RsaPrivateKey;

    /// Independent vector: `printf '{"pageNo":1,"pageSize":20,"status":0}' | openssl enc -aes-128-cbc -K … -iv …`
    #[test]
    fn signed_payload_matches_an_independent_aes_cbc_vector() {
        let key = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let payload = serde_json::json!({"status": 0, "pageNo": 1, "pageSize": 20});
        let encoded = encode_query(&payload, &key).unwrap();
        assert_eq!(
            encoded,
            "VdMwXK4ldoXH8MWI9qR8/qHbhcwvW2DNFfM38j/bLLLQdynaaZIMDkQ7VS4MvFdF"
        );
    }

    /// The reply is an RSA private-key operation with type-1 padding; we undo it with the public
    /// key. Generate a key, sign a synthetic `{"cKey":…}` block, recover it.
    #[test]
    fn recovers_the_gateway_key_from_a_signed_block() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        let private = RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let public = RsaPublicKey::from(&private);

        let body = br#"{"cKey":"0123456789abcdef"}"#;
        let mut block = vec![0x00, 0x01];
        block.extend(std::iter::repeat_n(
            0xFF,
            public.size() - body.len() - 3,
        ));
        block.push(0x00);
        block.extend_from_slice(body);
        assert_eq!(block.len(), public.size());

        let message = BigUint::from_bytes_be(&block);
        let signed = message.modpow(private.d(), private.n()).to_bytes_be();
        let mut padded = vec![0u8; public.size() - signed.len()];
        padded.extend(signed);

        let recovered = recover_protocol_key(&public, &STANDARD.encode(padded)).unwrap();
        assert_eq!(recovered, b"0123456789abcdef");
    }

    #[test]
    fn rejects_a_block_whose_padding_is_not_type_one() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(11);
        let private = RsaPrivateKey::new(&mut rng, 1024).unwrap();
        let public = RsaPublicKey::from(&private);
        let block = vec![0xFF; public.size()];
        let message = BigUint::from_bytes_be(&block);
        let signed = message.modpow(private.d(), private.n()).to_bytes_be();
        let mut padded = vec![0u8; public.size() - signed.len()];
        padded.extend(signed);
        assert!(recover_protocol_key(&public, &STANDARD.encode(padded)).is_err());
    }

    #[test]
    fn bootstrap_key_parses_and_encrypts_a_module_request() {
        let key = bootstrap_public_key().unwrap();
        let encoded = encrypt_module_request(&key, 13).unwrap();
        assert_eq!(STANDARD.decode(encoded).unwrap().len(), key.size());
    }
}
