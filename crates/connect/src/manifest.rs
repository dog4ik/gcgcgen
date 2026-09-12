//! The platform's gateway registration payload.

use serde::{Deserialize, Serialize};

use crate::method::MethodKind;

/// The `params_fields` block for one method: which keys the platform should
/// place in each inbound bucket.
///
/// Deriving this from the integration's own expressions keeps the registration
/// honest — a field the spec reads is a field the platform is asked to send.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub payment: Vec<String>,
    pub params: Vec<String>,
    pub settings: Vec<String>,
}

/// A method's entry in the registration document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodManifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_status_checker: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_waiting_seconds: Option<u32>,
    pub params_fields: Manifest,
}

/// The registration document posted to the platform's `gateway_settings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewayRegistration {
    pub gateway_key: String,
    pub full_link: String,
    pub enable: bool,
    pub callback: bool,
    pub processing_method: String,
    pub methods: std::collections::BTreeMap<MethodKind, MethodManifest>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serialises_in_the_platform_shape() {
        let reg = GatewayRegistration {
            gateway_key: "scripay".into(),
            full_link: "http://gcgcgen:4323".into(),
            enable: true,
            callback: true,
            processing_method: "http_requests".into(),
            methods: std::collections::BTreeMap::from([(
                MethodKind::Pay,
                MethodManifest {
                    enable_status_checker: Some(true),
                    final_waiting_seconds: Some(10),
                    params_fields: Manifest {
                        payment: vec!["token".into(), "gateway_amount".into()],
                        params: vec!["phone".into()],
                        settings: vec!["client_id".into()],
                    },
                },
            )]),
        };
        let v = serde_json::to_value(&reg).unwrap();
        assert_eq!(
            v["methods"]["pay"]["params_fields"]["params"],
            json!(["phone"])
        );
        assert_eq!(v["processing_method"], json!("http_requests"));
    }
}
