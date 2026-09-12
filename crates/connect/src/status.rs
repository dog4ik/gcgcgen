//! The canonical outcome vocabulary.

use serde::{Deserialize, Serialize};

/// The only three outcomes the platform understands.
///
/// The mapping from a gateway's own vocabulary onto these is per-integration
/// configuration; the one rule that is not configurable is that an *uncertain*
/// failure — a transport error, a 5xx, an unparseable body — must become
/// [`Status::Pending`] and never [`Status::Declined`], because the gateway may
/// have taken the money.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Approved,
    Declined,
    Pending,
}

impl Status {
    pub const ALL: [Status; 3] = [Status::Approved, Status::Declined, Status::Pending];

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Approved => "approved",
            Status::Declined => "declined",
            Status::Pending => "pending",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Status::ALL.into_iter().find(|v| v.as_str() == s)
    }

    /// Whether the transaction has reached a terminal state.
    pub fn is_final(self) -> bool {
        !matches!(self, Status::Pending)
    }
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_every_variant() {
        for s in Status::ALL {
            assert_eq!(Status::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{s}\""));
        }
        assert_eq!(Status::parse("succeeded"), None);
    }

    #[test]
    fn only_pending_is_non_final() {
        assert!(Status::Approved.is_final());
        assert!(Status::Declined.is_final());
        assert!(!Status::Pending.is_final());
    }
}
