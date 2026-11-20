alter table user
    add column armed_stream_id varchar(50),
    add constraint fk_user_armed_stream
        foreign key (armed_stream_id) references user_stream (id) on delete set null;
