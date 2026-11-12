-- 0011_sponsors (up) — restore the upstream sponsor domain (user decision 2026-11-13).
-- Upstream: `sponsors` table + priority-ascending match, later match overrides,
-- default_sponsor_text as the fallback. We store the filter as typed AST JSON
-- (plan §9.2 shape) instead of an expr-lang string — same engine as profile filters.
CREATE TABLE sponsors (
    id         uuid PRIMARY KEY,
    name       varchar(128) NOT NULL,
    text       text NOT NULL,
    filter_ast jsonb,
    priority   integer NOT NULL DEFAULT 0,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_sponsors_priority ON sponsors (priority ASC);
