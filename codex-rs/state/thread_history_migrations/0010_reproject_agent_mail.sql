-- The next read replays retained rollouts for newly visible mail and exec failures.
-- Existing item IDs are unchanged. The canonical rollout is not modified.
ALTER TABLE thread_history_projection_state ADD COLUMN pending_exec_calls TEXT;
DELETE FROM thread_history_projection_state;
