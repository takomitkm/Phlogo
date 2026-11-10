//! Shared application state for handlers.
use database::{NodeRepository, SourceRepository};
use registry::{NodeService, SourceService};

#[derive(Clone)]
pub struct AppState {
    pub sources: SourceService,
    pub nodes: NodeService,
    pool: sqlx::PgPool,
}

impl AppState {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            sources: SourceService::new(SourceRepository { pool: pool.clone() }),
            nodes: NodeService::new(NodeRepository { pool: pool.clone() }),
            pool,
        }
    }

    /// Shared pool access for simple handlers (config KV etc.).
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
}
