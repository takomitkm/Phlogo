//! Phlogo Wish Gnosis worker library: Apalis job definitions + idempotent side effects
//! + the fetch closed loop (P2) and periodic scheduling entry points.
pub mod check_job;
pub mod fetch_job;

pub use check_job::{finish_check_run, handle_check, record_check_run, CheckJob};
pub use fetch_job::{
    enqueue_for_active_sources, handle_fetch, run_fetch, trigger_fetch_batch, FetchJob,
};

/// Derive the Apalis (job-queue) DSN from the main DSN unless explicitly provided.
/// The queue uses its OWN database so apalis's embedded sqlx migrations never share
/// the `_sqlx_migrations` history of our business migrations (plan §3.1 separation).
pub fn jobs_dsn(cfg: &app_config::Config) -> String {
    if let Ok(v) = std::env::var("PHLOGO_DATABASE_JOBS_DSN") {
        if !v.is_empty() {
            return v;
        }
    }
    let main = &cfg.database.dsn;
    match main.rfind('/') {
        Some(pos) => format!("{}{}", &main[..pos + 1], "phlogo_jobs"),
        None => main.clone(),
    }
}
