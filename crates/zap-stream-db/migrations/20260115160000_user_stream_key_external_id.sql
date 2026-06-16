ALTER TABLE user_stream_key
    ADD column external_id varchar(128);

CREATE INDEX idx_user_stream_key_external_id ON user_stream_key (external_id);
