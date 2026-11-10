//! Registry layer: domain services over repositories (mirrors Phlogo
//! `internal/source/service.go` and `internal/node/*`: orchestration + dedup decisions).
pub mod node;
pub mod reconcile;
pub mod source;

pub use node::NodeService;
pub use reconcile::{
    delete_source, reconcile_fetch_result, source_ctx, FetchRunSummary, ReconcileError,
};
pub use source::{CreatedSource, SourceService};
