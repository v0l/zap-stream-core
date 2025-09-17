ALTER TABLE user ADD COLUMN nwc TEXT;
ALTER TABLE payment
    ADD COLUMN external_data TEXT,
    ADD COLUMN expires timestamp not null default current_timestamp;