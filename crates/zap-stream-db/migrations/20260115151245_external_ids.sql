ALTER TABLE user
    ADD column external_id varchar(128),
    ADD constraint fk_user_ingest_id
        foreign key (ingest_id) references ingest_endpoint(id);
ALTER TABLE user_stream
    ADD column external_id varchar(128);
ALTER TABLE user_stream_forward
    ADD column external_id varchar(128);
CREATE INDEX idx_user_external_id ON user (external_id);
CREATE INDEX idx_user_stream_external_id ON user_stream (external_id);
CREATE INDEX idx_user_stream_forward_external_id ON user_stream_forward (external_id);