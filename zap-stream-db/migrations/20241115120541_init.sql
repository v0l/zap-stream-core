-- Add migration script here
create table user
(
    id           integer unsigned not null auto_increment primary key,
    pubkey       binary(32) not null,
    created      timestamp       default current_timestamp,
    balance      bigint not null default 0,
    tos_accepted timestamp,
    stream_key   text   not null default uuid(),
    is_admin     bool   not null default false,
    is_blocked   bool   not null default false
);
create unique index ix_user_pubkey on user (pubkey);
create table user_stream
(
    id              UUID not null primary key,
    user_id         integer unsigned not null,
    starts          timestamp not null,
    ends            timestamp,
    state           smallint  not null,
    title           text,
    summary         text,
    image           text,
    thumb           text,
    tags            text,
    content_warning text,
    goal            text,
    pinned          text,
    -- milli-sats paid for this stream
    cost            bigint    not null default 0,
    -- duration in seconds
    duration        float     not null default 0,
    -- admission fee
    fee             integer unsigned,
    -- current nostr event json
    event           text,

    constraint fk_user_stream_user
        foreign key (user_id) references user (id)
);