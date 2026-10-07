//! Source documents are editable; runtime configs contain expanded hotkeys.
mod document;
mod parse;
mod types;
mod schema;
pub use schema::*;
pub use document::ConfigDocument;
pub use types::*;
#[cfg(test)]
mod tests;
