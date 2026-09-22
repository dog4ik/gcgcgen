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

fn encrypt_merchant_key(merchant_key: &str, sign_key: &[u8; 32], iv: [u8; 16]) -> (String, String) {
    use aes::cipher::{BlockModeEncrypt, KeyIvInit};
    let ciphertext = cbc::Encryptor::<aes::Aes256>::new(&(*sign_key).into(), &iv.into())
        .encrypt_padded_vec::<block_padding::Pkcs7>(merchant_key.as_bytes());
    (STANDARD.encode(ciphertext), STANDARD.encode(iv))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGN_KEY: &[u8; 32] = b"e7403b3c0d76a35312e7cc65eeb75808";

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
}
