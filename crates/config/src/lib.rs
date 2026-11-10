//! Configuration layer: Figment-backed config with `PAILY_` env overrides.
//! Shape mirrors Phlogo `config.example.yaml` (server/database/auth), CONVENTIONS §2.6.
use figment::{
    providers::{Format, Serialized, Yaml},
    Figment,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub cors_origins: Vec<String>,
    pub verbose: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(rename = "type")]
    pub db_type: String,
    pub dsn: String,
    pub max_open_conns: u32,
    pub max_idle_conns: u32,
    pub conn_max_lifetime_minutes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    pub admin_password: String,
    pub service_secret: String,
    pub jwt_secret: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 21380,
            cors_origins: Vec::new(),
            verbose: false,
        }
    }
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            db_type: "postgres".into(),
            dsn: "postgres://paily:change-me@localhost:5432/paily".into(),
            max_open_conns: 6,
            max_idle_conns: 2,
            conn_max_lifetime_minutes: 60,
        }
    }
}

impl Config {
    /// Load config: defaults -> optional YAML -> `PAILY_` env overrides.
    #[allow(clippy::result_large_err)]
    pub fn load(path: Option<&str>) -> Result<Self, figment::Error> {
        let mut fig = Figment::from(Serialized::defaults(Config::default()));
        let candidate = path
            .map(|s| s.to_string())
            .or_else(|| std::env::var("PAILY_CONFIG").ok());
        if let Some(pf) = candidate {
            if std::path::Path::new(&pf).exists() {
                fig = fig.merge(Yaml::file(pf));
            }
        } else if std::path::Path::new("config.yaml").exists() {
            fig = fig.merge(Yaml::file("config.yaml"));
        }
        let mut cfg: Config = fig.extract()?;
        cfg.apply_env();
        Ok(cfg)
    }

    /// Explicit `PAILY_` env overlay (server.port -> PAILY_SERVER_PORT, etc.).
    fn apply_env(&mut self) {
        use std::env::var;
        if let Ok(v) = var("PAILY_SERVER_PORT") {
            if let Ok(n) = v.parse() {
                self.server.port = n;
            }
        }
        if let Ok(v) = var("PAILY_SERVER_VERBOSE") {
            self.server.verbose = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes");
        }
        if let Ok(v) = var("PAILY_DATABASE_TYPE") {
            self.database.db_type = v;
        }
        if let Ok(v) = var("PAILY_DATABASE_DSN") {
            self.database.dsn = v;
        }
        if let Ok(v) = var("PAILY_AUTH_ADMIN_PASSWORD") {
            self.auth.admin_password = v;
        }
        if let Ok(v) = var("PAILY_AUTH_SERVICE_SECRET") {
            self.auth.service_secret = v;
        }
        if let Ok(v) = var("PAILY_AUTH_JWT_SECRET") {
            self.auth.jwt_secret = v;
        }
    }
}
