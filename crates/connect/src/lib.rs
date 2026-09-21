//! The gateway-connect contract with reactivepay.

pub mod callback;
pub mod log;
pub mod manifest;
pub mod method;
pub mod request;
pub mod response;
pub mod status;

pub use callback::{CallbackPayload, CallbackStatus};
pub use log::{InteractionLog, LoggedRequest};
pub use manifest::Manifest;
pub use method::MethodKind;
pub use request::ConnectInput;
pub use response::{ConnectResponse, Iframe, RedirectRequest, TransactionResponse};
pub use status::Status;
