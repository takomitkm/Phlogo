//! Shared application state for handlers.
use database::{NodeRepository, SourceRepository};
use registry::{NodeService, SourceService};

#[derive(Clone)]
pub struct AppState {
    pub sources: SourceService,
    pub nodes: NodeService,
}

impl AppState {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            sources: SourceService::new(SourceRepository { pool: pool.clone() }),
            nodes: NodeService::new(NodeRepository { pool }),
        }
    }
}
