-- Add input_uid column to user_stream to map external inputs to streams
ALTER TABLE user_stream ADD COLUMN input_uid TEXT;
