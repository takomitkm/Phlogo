-- First-boot initialization: the Apalis job queue lives in its own database so its
-- embedded sqlx migrations never share the business `_sqlx_migrations` history
-- (plan §3.1). POSTGRES_DB only creates `phlogo`; this script adds the queue database.
CREATE DATABASE phlogo_jobs;
GRANT ALL PRIVILEGES ON DATABASE phlogo_jobs TO phlogo;
