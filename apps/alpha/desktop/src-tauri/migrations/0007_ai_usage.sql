CREATE TABLE ai_usage_events (
  id TEXT PRIMARY KEY,
  request_id TEXT NOT NULL UNIQUE,
  provider TEXT NOT NULL CHECK (provider IN ('codex', 'claude')),
  auth_mode TEXT NOT NULL CHECK (auth_mode IN ('subscription', 'api_key')),
  model TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
  cached_input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
  cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cache_creation_input_tokens >= 0),
  output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
  reasoning_output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (reasoning_output_tokens >= 0),
  estimated_cost_usd_micros INTEGER CHECK (estimated_cost_usd_micros >= 0),
  cost_source TEXT CHECK (cost_source IN ('provider_estimate', 'local_price_table')),
  pricing_version TEXT,
  occurred_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE INDEX idx_ai_usage_provider_auth_time
  ON ai_usage_events (provider, auth_mode, occurred_at DESC);
