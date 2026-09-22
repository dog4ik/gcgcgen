use serde::{Deserialize, Serialize};

use crate::method::MethodKind;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub payment: Vec<String>,
    pub refund: Vec<String>,
    pub params: Vec<String>,
    pub settings: Vec<String>,
}

/// A method in the settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MethodManifest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_status_checker: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_waiting_seconds: Option<u32>,
    pub params_fields: Manifest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewaySettings {
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
        let reg = GatewaySettings {
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
                        refund: vec!["token".into(), "gateway_amount".into()],
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
