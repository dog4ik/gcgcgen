use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodKind {
    Pay,
    Payout,
    Refund,
    Status,
}

impl MethodKind {
    pub const ALL: [MethodKind; 4] = [
        MethodKind::Pay,
        MethodKind::Payout,
        MethodKind::Refund,
        MethodKind::Status,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MethodKind::Pay => "pay",
            MethodKind::Payout => "payout",
            MethodKind::Refund => "refund",
            MethodKind::Status => "status",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        MethodKind::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

impl std::fmt::Display for MethodKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn round_trips_as_a_map_key() {
        let m = BTreeMap::from([(MethodKind::Pay, 1), (MethodKind::Status, 2)]);
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, r#"{"pay":1,"status":2}"#);
        assert_eq!(
            serde_json::from_str::<BTreeMap<MethodKind, i32>>(&s).unwrap(),
            m
        );
    }

    #[test]
    fn parses_every_variant() {
        for m in MethodKind::ALL {
            assert_eq!(MethodKind::parse(m.as_str()), Some(m));
        }
        assert_eq!(MethodKind::parse("capture"), None);
    }
}
