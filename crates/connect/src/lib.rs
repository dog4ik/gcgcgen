//! The gateway-connect contract with reactivepay.
//!
//! Everything in this crate is fixed by the platform, not by any particular
//! gateway: the three-bucket request envelope, the canonical
//! `approved | declined | pending` vocabulary, the response envelope, and the
//! interaction log returned inline with every reply.
//!
//! It is deliberately free of engine machinery — no HTTP client, no clock, no
//! async — so it compiles to wasm for the editor UI and can equally back a
//! hand-written adapter that bypasses the spec engine entirely.
//!
//! Two conventions are easy to get wrong and are encoded here:
//!
//! * **Failures are HTTP 200.** An error is `{"result": false, "error": …}`
//!   with a 200 status; a non-200 makes the platform retry rather than record
//!   the failure. See [`ConnectResponse`].
//! * **Credentials are per-request.** They arrive in [`ConnectInput::settings`]
//!   and are never persisted by this service.

pub mod log;
pub mod manifest;
pub mod method;
pub mod request;
pub mod response;
pub mod status;

pub use log::{InteractionLog, LoggedRequest};
pub use manifest::Manifest;
pub use method::MethodKind;
pub use request::ConnectInput;
pub use response::{ConnectResponse, TransactionResponse};
pub use status::Status;
