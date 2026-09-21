//! Forwarding a callback to reactivepay.
//!
//! `POST {business_url}/callbacks/v2/gateway_callbacks/{payment.token}` with
//! the payload as JSON and a Bearer JWT (HS512, keyed by `SIGN_KEY`). The JWT
//! repeats the payload and adds a `secure` block: the merchant's private key,
//! AES-256-CBC encrypted with the same key.

use aes::cipher::block_padding;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use serde::Serialize;

use connect::CallbackPayload;

/// Where and how callbacks are forwarded.
#[derive(Debug, Clone)]
pub struct PlatformConfig {
    pub business_url: String,
    pub sign_key: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("could not sign the callback: {0}")]
    Sign(String),
    #[error("platform rejected the callback: {0}")]
    Send(#[from] reqwest::Error),
}

pub async fn send_callback(
    client: &reqwest::Client,
    config: &PlatformConfig,
    token: &str,
    merchant_private_key: &str,
    payload: &CallbackPayload,
) -> Result<(), ForwardError> {
    let jwt = create_jwt(payload, merchant_private_key, &config.sign_key)?;
    let url = format!(
        "{}/callbacks/v2/gateway_callbacks/{token}",
        config.business_url.trim_end_matches('/')
    );
    client
        .post(&url)
        .bearer_auth(jwt)
        .json(payload)
        .send()
        .await
        .and_then(|r| r.error_for_status())?;
    Ok(())
}

#[derive(Serialize)]
struct SecureBlock {
    encrypted_data: String,
    iv_value: String,
}

#[derive(Serialize)]
struct Claims<'a> {
    #[serde(flatten)]
    payload: &'a CallbackPayload,
    secure: SecureBlock,
}

/// The signed JWT: the payload plus the encrypted merchant key, HS512.
pub fn create_jwt(
    payload: &CallbackPayload,
    merchant_private_key: &str,
    sign_key: &[u8; 32],
) -> Result<String, ForwardError> {
    use rand::Rng;
    let mut iv = [0u8; 16];
    rand::rng().fill_bytes(&mut iv);
    let (encrypted_data, iv_value) = encrypt_merchant_key(merchant_private_key, sign_key, iv);

    let claims = Claims {
        payload,
        secure: SecureBlock {
            encrypted_data,
            iv_value,
        },
    };
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"HS512"}"#);
    let body = serde_json::to_vec(&claims).map_err(|e| ForwardError::Sign(e.to_string()))?;
    let signing_input = format!("{header}.{}", URL_SAFE_NO_PAD.encode(body));
    let signature = URL_SAFE_NO_PAD.encode(hs512(sign_key, signing_input.as_bytes()));
    Ok(format!("{signing_input}.{signature}"))
}

fn hs512(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, KeyInit, Mac};
    let mut mac =
        <Hmac<sha2::Sha512>>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// AES-256-CBC/PKCS7 with `sign_key`; base64 ciphertext and IV.
fn encrypt_merchant_key(merchant_key: &str, sign_key: &[u8; 32], iv: [u8; 16]) -> (String, String) {
    use aes::cipher::{BlockModeEncrypt, KeyIvInit};
    let ciphertext = cbc::Encryptor::<aes::Aes256>::new(&(*sign_key).into(), &iv.into())
        .encrypt_padded_vec::<block_padding::Pkcs7>(merchant_key.as_bytes());
    (STANDARD.encode(ciphertext), STANDARD.encode(iv))
}

#[cfg(test)]
mod tests {
    use super::*;
    use connect::CallbackStatus;
    use serde_json::Value;

    const SIGN_KEY: &[u8; 32] = b"e7403b3c0d76a35312e7cc65eeb75808";

    /// The known answer from the hand-written adapter, so the two agree byte
    /// for byte on what the platform decrypts.
    #[test]
    fn encrypts_the_merchant_key_like_oxyscripay() {
        let iv: [u8; 16] = hex::decode("293c20e6038619aa40d774f4fc6934f2")
            .unwrap()
            .try_into()
            .unwrap();
        let (data, iv_b64) = encrypt_merchant_key("5178831496700b3634e4", SIGN_KEY, iv);
        assert_eq!(data, "cZu0SLPSItrNtfG8hVIz24Dc6eHW1Ujj19LFGD7t6yk=");
        assert_eq!(iv_b64, STANDARD.encode(iv));
    }

    #[test]
    fn the_jwt_is_hs512_over_the_payload_and_the_secure_block() {
        let payload = CallbackPayload {
            status: CallbackStatus::Declined {
                reason: "Failed".into(),
            },
            currency: "KES".into(),
            amount: 1000,
            logs: vec![],
        };
        let jwt = create_jwt(&payload, "5178831496700b3634e4", SIGN_KEY).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);

        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "HS512");

        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["status"], "declined");
        assert_eq!(claims["reason"], "Failed");
        assert_eq!(claims["amount"], 1000);
        assert!(claims["secure"]["encrypted_data"].is_string());
        assert!(claims["secure"]["iv_value"].is_string());

        let expected = hs512(SIGN_KEY, format!("{}.{}", parts[0], parts[1]).as_bytes());
        assert_eq!(URL_SAFE_NO_PAD.decode(parts[2]).unwrap(), expected);
    }
}
