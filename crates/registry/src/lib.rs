//! Registry layer: domain services over repositories (mirrors Phlogo
//! `internal/source/service.go` and `internal/node/*`: orchestration + dedup decisions).
pub mod node;
pub mod source;

pub use node::NodeService;
pub use source::{CreatedSource, SourceService};
