-- First-boot initialization: the Apalis job queue lives in its own database so its
-- embedded sqlx migrations never share the business `_sqlx_migrations` history
-- (plan §3.1). POSTGRES_DB only creates `paily`; this script adds the queue database.
CREATE DATABASE paily_jobs;
GRANT ALL PRIVILEGES ON DATABASE paily_jobs TO paily;
