-- ADD FK TO TRACK IF STREAM IS USING FIXED STREAM KEY
-- REMOVE UNUSED LAST_SEGMENT COL
ALTER TABLE user_stream
    ADD COLUMN stream_key_id integer unsigned,
    ADD constraint fk_stream_stream_key
        foreign key (stream_key_id) references user_stream_key (id),
    DROP COLUMN last_segment;