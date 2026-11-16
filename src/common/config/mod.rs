//! Source documents are editable; runtime configs contain expanded hotkeys.
mod document;
mod parse;
mod schema;
mod types;
pub use document::ConfigDocument;
pub use schema::*;
pub use types::*;
#[cfg(test)]
mod tests;
