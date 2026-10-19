//! Owner: core agent. Parses PT1H.json blobs and holds the blob path conventions.
pub mod parse;
pub mod paths;

pub use parse::parse_flow_log;
