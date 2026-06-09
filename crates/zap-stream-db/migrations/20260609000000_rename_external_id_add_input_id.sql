ALTER TABLE user_stream CHANGE COLUMN external_id external_video_id VARCHAR(128);
DROP INDEX idx_user_stream_external_id ON user_stream;
ALTER TABLE user_stream ADD COLUMN external_input_id VARCHAR(128);
CREATE INDEX idx_user_stream_external_input_id ON user_stream (external_input_id);
