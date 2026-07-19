CREATE TABLE IF NOT EXISTS nodes (
    id UUID PRIMARY KEY,
    record JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS sandboxes (
    id UUID PRIMARY KEY,
    tenant TEXT NOT NULL,
    record JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS sandboxes_tenant_updated_idx ON sandboxes (tenant, updated_at DESC);
CREATE INDEX IF NOT EXISTS sandboxes_record_gin_idx ON sandboxes USING GIN (record jsonb_path_ops);

CREATE TABLE IF NOT EXISTS operations (
    id UUID PRIMARY KEY,
    sandbox_id UUID NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    record JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS operations_sandbox_idx ON operations (sandbox_id, updated_at DESC);

CREATE TABLE IF NOT EXISTS assignments (
    id UUID PRIMARY KEY,
    node_id UUID NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    operation_id UUID NOT NULL REFERENCES operations(id) ON DELETE CASCADE,
    sandbox_id UUID NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    state TEXT NOT NULL CHECK (state IN ('pending', 'leased', 'completed', 'failed')),
    lease_until TIMESTAMPTZ,
    record JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS assignments_lease_idx ON assignments (node_id, state, lease_until, created_at);
