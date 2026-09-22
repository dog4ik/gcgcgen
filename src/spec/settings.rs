use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsSchema {
    #[serde(default)]
    pub fields: Vec<FieldDef>,
}

impl SettingsSchema {
    /// Field names for the platform's `params_fields.settings` manifest.
    pub fn manifest(&self) -> Vec<String> {
        self.fields.iter().map(|f| f.name.clone()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    /// Key within the gc `settings` object.
    pub name: String,
    /// Masked in the editor and redacted from interaction logs.
    #[serde(default)]
    pub secret: bool,
}

impl FieldDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            secret: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let schema = SettingsSchema {
            fields: vec![
                FieldDef::new("client_id"),
                FieldDef {
                    name: "client_secret".into(),
                    secret: true,
                },
            ],
        };
        let json = serde_json::to_string(&schema).unwrap();
        assert_eq!(
            serde_json::from_str::<SettingsSchema>(&json).unwrap(),
            schema
        );
        assert_eq!(schema.manifest(), ["client_id", "client_secret"]);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let err = serde_json::from_str::<FieldDef>(r#"{"name":"a","secrt":true}"#).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{err}");
    }
}
