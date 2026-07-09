-- Pulsar MultiEdit Database Schema
-- PostgreSQL 14+

-- Enable UUID extension
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

-- Sessions table
CREATE TABLE IF NOT EXISTS sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) UNIQUE NOT NULL,
    host_id VARCHAR(255) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    closed_at TIMESTAMPTZ,
    status VARCHAR(50) NOT NULL DEFAULT 'active',
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    CONSTRAINT valid_status CHECK (status IN ('active', 'closed', 'expired'))
);

-- Indexes for sessions
CREATE INDEX IF NOT EXISTS idx_sessions_session_id ON sessions(session_id);
CREATE INDEX IF NOT EXISTS idx_sessions_host_id ON sessions(host_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_sessions_created_at ON sessions(created_at DESC);

-- Snapshots table
CREATE TABLE IF NOT EXISTS snapshots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    snapshot_id VARCHAR(255) UNIQUE NOT NULL,
    session_id VARCHAR(255) NOT NULL,
    s3_key VARCHAR(1024) NOT NULL,
    s3_bucket VARCHAR(255) NOT NULL,
    size_bytes BIGINT NOT NULL,
    compressed BOOLEAN NOT NULL DEFAULT false,
    hash BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE
);

-- Indexes for snapshots
CREATE INDEX IF NOT EXISTS idx_snapshots_session_id ON snapshots(session_id);
CREATE INDEX IF NOT EXISTS idx_snapshots_snapshot_id ON snapshots(snapshot_id);
CREATE INDEX IF NOT EXISTS idx_snapshots_created_at ON snapshots(created_at DESC);

-- Optional: Participants table for tracking session participants
CREATE TABLE IF NOT EXISTS participants (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) NOT NULL,
    peer_id VARCHAR(255) NOT NULL,
    role VARCHAR(50) NOT NULL,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    left_at TIMESTAMPTZ,
    last_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    FOREIGN KEY (session_id) REFERENCES sessions(session_id) ON DELETE CASCADE,
    UNIQUE(session_id, peer_id)
);

-- Indexes for participants
CREATE INDEX IF NOT EXISTS idx_participants_session_id ON participants(session_id);
CREATE INDEX IF NOT EXISTS idx_participants_peer_id ON participants(peer_id);
CREATE INDEX IF NOT EXISTS idx_participants_active ON participants(session_id, peer_id)
    WHERE left_at IS NULL;

-- Function to auto-expire sessions
CREATE OR REPLACE FUNCTION expire_old_sessions()
RETURNS INTEGER AS $$
DECLARE
    expired_count INTEGER;
BEGIN
    UPDATE sessions
    SET status = 'expired', closed_at = NOW()
    WHERE status = 'active' AND expires_at <= NOW();

    GET DIAGNOSTICS expired_count = ROW_COUNT;
    RETURN expired_count;
END;
$$ LANGUAGE plpgsql;

-- Optional: Create a scheduled job to expire sessions
-- (Requires pg_cron extension)
-- SELECT cron.schedule('expire-sessions', '*/5 * * * *', 'SELECT expire_old_sessions();');

-- Grant permissions (adjust as needed)
-- GRANT SELECT, INSERT, UPDATE, DELETE ON sessions TO pulsar_app;
-- GRANT SELECT, INSERT, UPDATE, DELETE ON snapshots TO pulsar_app;
-- GRANT SELECT, INSERT, UPDATE, DELETE ON participants TO pulsar_app;
-- GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO pulsar_app;

-- Comments for documentation
COMMENT ON TABLE sessions IS 'Collaborative editing sessions';
COMMENT ON TABLE snapshots IS 'Session state snapshots stored in S3';
COMMENT ON TABLE participants IS 'Session participants and their roles';

COMMENT ON COLUMN sessions.session_id IS 'Unique session identifier';
COMMENT ON COLUMN sessions.host_id IS 'ID of the session host/creator';
COMMENT ON COLUMN sessions.status IS 'Session status: active, closed, or expired';
COMMENT ON COLUMN sessions.metadata IS 'Arbitrary session metadata as JSON';

COMMENT ON COLUMN snapshots.s3_key IS 'S3 object key for the snapshot data';
COMMENT ON COLUMN snapshots.hash IS 'SHA-256 hash of snapshot data for integrity verification';

COMMENT ON COLUMN participants.role IS 'Participant role: host, editor, or observer';
COMMENT ON COLUMN participants.last_seen IS 'Last heartbeat timestamp';
