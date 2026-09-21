//! Engine failures and the uncertainty rule.

use crate::spec::EvalError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum EngineError {
    /// The request never completed: DNS, connect, TLS, timeout, reset.
    #[error("transport error: {0}")]
    Transport(String),
    /// A response arrived but could not be understood.
    #[error("invalid response: {0}")]
    Decode(String),
    /// The gateway answered, and said no.
    #[error("{message}")]
    Api { message: String, status: u16 },
    /// An expression failed while rendering or interpreting.
    #[error("{0}")]
    Eval(#[from] EvalError),
    /// The document is wrong in a way validation did not catch.
    #[error("{0}")]
    Config(String),
}

impl EngineError {
    /// Whether the gateway may have acted despite the failure — the single
    /// most consequential rule in the engine. Uncertain means the method
    /// reports `pending` and the platform's poller settles it; reporting
    /// `declined` would mark a possibly-charged payment as failed.
    pub fn is_uncertain(&self) -> bool {
        match self {
            EngineError::Transport(_) | EngineError::Decode(_) => true,
            EngineError::Api { status, .. } => *status >= 500,
            EngineError::Eval(_) | EngineError::Config(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertainty_matrix() {
        assert!(EngineError::Transport("reset".into()).is_uncertain());
        assert!(EngineError::Decode("not json".into()).is_uncertain());
        assert!(EngineError::Api {
            message: "boom".into(),
            status: 500
        }
        .is_uncertain());
        assert!(EngineError::Api {
            message: "boom".into(),
            status: 503
        }
        .is_uncertain());
        assert!(!EngineError::Api {
            message: "bad request".into(),
            status: 400
        }
        .is_uncertain());
        assert!(!EngineError::Api {
            message: "denied".into(),
            status: 403
        }
        .is_uncertain());
        assert!(!EngineError::Config("nope".into()).is_uncertain());
    }
}
