-- 0009_subscriptions (up) — P5-03 profile/token tables (plan §5.1/§10.2).
CREATE TABLE subscription_profiles (
    id             uuid PRIMARY KEY,
    name           varchar(128) NOT NULL,
    filter_ast     jsonb,
    region         varchar(8),
    health         varchar(16) NOT NULL DEFAULT 'alive',
    min_score      double precision NOT NULL DEFAULT 0,
    limit_count    integer NOT NULL DEFAULT 200,
    order_strategy varchar(32) NOT NULL DEFAULT 'score_desc',
    preset         varchar(16) NOT NULL DEFAULT 'common',
    output_format  varchar(16) NOT NULL,
    naming_rule    text,
    enabled        boolean NOT NULL DEFAULT true,
    version        bigint NOT NULL DEFAULT 1
);

CREATE TABLE subscription_tokens (
    id         uuid PRIMARY KEY,
    profile_id uuid NOT NULL REFERENCES subscription_profiles (id) ON DELETE CASCADE,
    token_hash varchar(128) NOT NULL UNIQUE,
    status     varchar(16) NOT NULL DEFAULT 'active',
    created_at timestamptz NOT NULL DEFAULT now(),
    revoked_at timestamptz
);
