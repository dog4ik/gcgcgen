use serde_json::{Map, Value};

use crate::spec::MethodKind;
pub use connect::ConnectInput;

/// Runtime facts injected under the `env` root.
#[derive(Debug, Clone)]
pub struct Env {
    pub integration_key: String,
    pub base_url: String,
    pub callback_url: String,
    pub sandbox: bool,
    pub now_rfc3339: String,
    pub unix_now: i64,
    pub request_id: String,
}

impl Env {
    fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "integration_key".into(),
            Value::String(self.integration_key.clone()),
        );
        m.insert("base_url".into(), Value::String(self.base_url.clone()));
        m.insert(
            "callback_url".into(),
            Value::String(self.callback_url.clone()),
        );
        m.insert("sandbox".into(), Value::Bool(self.sandbox));
        m.insert("now".into(), Value::String(self.now_rfc3339.clone()));
        m.insert("unix_now".into(), Value::Number(self.unix_now.into()));
        m.insert("request_id".into(), Value::String(self.request_id.clone()));
        Value::Object(m)
    }
}

#[derive(Debug, Clone)]
pub struct Scope {
    root: Map<String, Value>,
}

impl Scope {
    pub fn new(input: &ConnectInput, env: &Env, method: MethodKind) -> Self {
        let mut root = Map::new();
        root.insert("payment".into(), input.payment.clone());
        root.insert("refund".into(), input.refund.clone());
        root.insert("params".into(), input.params.clone());
        root.insert("settings".into(), input.settings.clone());
        root.insert("steps".into(), Value::Object(Map::new()));
        root.insert("env".into(), env.to_value());
        root.insert(
            "method".into(),
            Value::Object(Map::from_iter([(
                "kind".to_string(),
                Value::String(method.to_string()),
            )])),
        );
        Self { root }
    }

    /// A callback scope
    pub fn for_callback(settings: &Value, callback: Value, env: &Env) -> Self {
        let mut root = Map::new();
        root.insert("settings".into(), settings.clone());
        root.insert("callback".into(), callback);
        root.insert("steps".into(), Value::Object(Map::new()));
        root.insert("env".into(), env.to_value());
        root.insert(
            "method".into(),
            Value::Object(Map::from_iter([(
                "kind".to_string(),
                Value::String("callback".into()),
            )])),
        );
        Self { root }
    }

    /// What a callback's `lookup` sees: the callback and `env`, nothing else.
    pub fn for_lookup(callback: Value, env: &Env) -> Value {
        Value::Object(Map::from_iter([
            ("callback".to_string(), callback),
            ("env".to_string(), env.to_value()),
        ]))
    }

    pub fn value(&self) -> Value {
        Value::Object(self.root.clone())
    }

    /// Records a completed request's output under `steps.<name>`.
    pub fn set_step(&mut self, name: &str, value: Value) {
        let Some(Value::Object(steps)) = self.root.get_mut("steps") else {
            unreachable!("steps is seeded as an object in `new`")
        };
        steps.insert(name.to_string(), value);
    }

    /// The scope a response spec sees: everything above plus `resp`.
    pub fn with_resp(&self, resp: Value) -> Value {
        let mut root = self.root.clone();
        root.insert("resp".into(), resp);
        Value::Object(root)
    }

    /// The scope a signature canonical string sees: everything plus `req`.
    pub fn with_req(&self, req: Value) -> Value {
        let mut root = self.root.clone();
        root.insert("req".into(), req);
        Value::Object(root)
    }

    /// Values of settings fields the schema marks `secret`, for log redaction.
    pub fn secret_values(&self, schema: &crate::spec::SettingsSchema) -> Vec<String> {
        let Some(Value::Object(settings)) = self.root.get("settings") else {
            return Vec::new();
        };
        schema
            .fields
            .iter()
            .filter(|f| f.secret)
            .filter_map(|f| settings.get(&f.name))
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{FieldDef, SettingsSchema};
    use serde_json::json;

    fn env() -> Env {
        Env {
            integration_key: "scripay".into(),
            base_url: "https://api.example.com".into(),
            callback_url: "https://cb.example/callback".into(),
            sandbox: false,
            now_rfc3339: "2026-09-06T00:00:00Z".into(),
            unix_now: 1_788_000_000,
            request_id: "req-1".into(),
        }
    }

    fn input() -> ConnectInput {
        ConnectInput {
            payment: json!({"token": "tok_1"}),
            refund: json!({"token": "tok_2"}),
            params: json!({"phone": "070"}),
            settings: json!({"client_id": "cid", "client_secret": "sk_live_abcdef"}),
        }
    }

    #[test]
    fn seeds_every_root() {
        let s = Scope::new(&input(), &env(), MethodKind::Pay);
        let v = s.value();
        for root in ["payment", "params", "settings", "steps", "env", "method"] {
            assert!(v.get(root).is_some(), "missing root {root}");
        }
        assert_eq!(v["method"]["kind"], json!("pay"));
        assert_eq!(v["env"]["unix_now"], json!(1_788_000_000));
    }

    #[test]
    fn steps_accumulate_and_are_readable() {
        let mut s = Scope::new(&input(), &env(), MethodKind::Payout);
        s.set_step("enquiry", json!({"ok": true, "body": {"reference": "R1"}}));
        assert_eq!(
            s.value()["steps"]["enquiry"]["body"]["reference"],
            json!("R1")
        );
        s.set_step("transfer", json!({"ok": true}));
        assert_eq!(s.value()["steps"]["transfer"]["ok"], json!(true));
    }

    #[test]
    fn resp_and_req_are_scoped_to_their_phase() {
        let s = Scope::new(&input(), &env(), MethodKind::Pay);
        assert!(s.value().get("resp").is_none());
        assert_eq!(
            s.with_resp(json!({"status": 200}))["resp"]["status"],
            json!(200)
        );
        assert_eq!(
            s.with_req(json!({"path": "/x"}))["req"]["path"],
            json!("/x")
        );
    }

    #[test]
    fn a_callback_scope_carries_only_settings_and_the_callback() {
        let s = Scope::for_callback(
            &json!({"client_id": "c"}),
            json!({"body": {"rrn": "R"}}),
            &env(),
        );
        let v = s.value();
        assert_eq!(v["callback"]["body"]["rrn"], json!("R"));
        assert_eq!(v["settings"]["client_id"], json!("c"));
        assert_eq!(v["method"]["kind"], json!("callback"));
        for absent in ["payment", "params", "refund"] {
            assert!(
                v.get(absent).is_none(),
                "{absent} is not kept for a callback"
            );
        }
    }

    #[test]
    fn collects_only_the_fields_marked_secret() {
        let schema = SettingsSchema {
            fields: vec![
                FieldDef::new("client_id"),
                FieldDef {
                    name: "client_secret".into(),
                    secret: true,
                },
            ],
        };
        let s = Scope::new(&input(), &env(), MethodKind::Pay);
        assert_eq!(s.secret_values(&schema), vec!["sk_live_abcdef".to_string()]);
    }
}
