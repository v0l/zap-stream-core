-- Add stream_dump_recording column to user table
ALTER TABLE user ADD COLUMN stream_dump_recording BOOL NOT NULL DEFAULT FALSE;