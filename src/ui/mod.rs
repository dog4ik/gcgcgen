//! The editor UI.
//!
//! Document validation runs in the browser: [`crate::spec::validate`] is pure
//! and compiles to wasm, so expression errors appear as they are typed with no
//! round trip. Only storage and the dry run call the server.

pub mod dry_run;
pub mod editor;
pub mod list;
pub mod widgets;

pub use editor::Editor;
pub use list::IntegrationList;
