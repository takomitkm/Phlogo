//! AppState with secrets accessors (P7 additions for auth/config endpoints).
use database::{NodeRepository, SourceRepository};
use registry::{NodeService, SourceService};

#[derive(Clone)]
pub struct AppState {
    pub sources: SourceService,
    pub nodes: NodeService,
    admin_password: String,
    jwt_secret: String,
    pool: sqlx::PgPool,
}

impl AppState {
    pub fn new(cfg: &app_config::Config, pool: sqlx::PgPool) -> Self {
        Self {
            sources: SourceService::new(SourceRepository { pool: pool.clone() }),
            nodes: NodeService::new(NodeRepository { pool: pool.clone() }),
            admin_password: cfg.auth.admin_password.clone(),
            jwt_secret: cfg.auth.jwt_secret.clone(),
            pool,
        }
    }

    /// Shared pool access for simple handlers (config KV etc.).
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }

    pub fn admin_password(&self) -> &str {
        &self.admin_password
    }

    pub fn jwt_secret(&self) -> &str {
        &self.jwt_secret
    }
}
