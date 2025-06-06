-- Add migration script here
create table user
(
    id              integer unsigned not null auto_increment primary key,
    pubkey          binary(32) not null,
    created         timestamp not null default current_timestamp,
    balance         bigint    not null default 0,
    tos_accepted    timestamp,
    stream_key      text      not null default uuid(),
    is_admin        bool      not null default false,
    is_blocked      bool      not null default false,
    recording       bool      not null default false,
    title           text,
    summary         text,
    image           text,
    tags            text,
    content_warning text,
    goal            text
);
create unique index ix_user_pubkey on user (pubkey);

-- Add ingest endpoints table for pipeline configuration (must come before user_stream)
create table ingest_endpoint
(
    id           integer unsigned not null auto_increment primary key,
    name         varchar(255) not null,
    cost         bigint unsigned not null default 10000,
    capabilities text
);

create table user_stream
(
    id              varchar(50) not null primary key,
    user_id         integer unsigned not null,
    starts          timestamp   not null,
    ends            timestamp,
    state           tinyint unsigned not null,
    title           text,
    summary         text,
    image           text,
    thumb           text,
    tags            text,
    content_warning text,
    goal            text,
    pinned          text,
    -- milli-sats paid for this stream
    cost            bigint unsigned    not null default 0,
    -- duration in seconds
    duration        float       not null default 0,
    -- admission fee
    fee             integer unsigned,
    -- current nostr event json
    event           text,
    -- endpoint id if using specific endpoint
    endpoint_id     integer unsigned,
    -- timestamp of last segment
    last_segment    timestamp,

    constraint fk_user_stream_user
        foreign key (user_id) references user (id),
    constraint fk_user_stream_endpoint
        foreign key (endpoint_id) references ingest_endpoint (id)
);

-- Add forwards table for payment forwarding
create table user_stream_forward
(
    id      integer unsigned not null auto_increment primary key,
    user_id integer unsigned not null,
    name    text not null,
    target  text not null,
    constraint fk_user_stream_forward_user
        foreign key (user_id) references user (id)
);

-- Add keys table for stream keys
create table user_stream_key
(
    id        integer unsigned not null auto_increment primary key,
    user_id   integer unsigned not null,
    `key`     text        not null,
    created   timestamp   not null default current_timestamp,
    expires   timestamp,
    stream_id varchar(50) not null,
    constraint fk_user_stream_key_user
        foreign key (user_id) references user (id),
    constraint fk_user_stream_key_stream
        foreign key (stream_id) references user_stream (id)
);

-- Add payments table for payment logging
create table payment
(
    payment_hash binary(32) not null primary key,
    user_id      integer unsigned not null,
    invoice      text,
    is_paid      bool      not null default false,
    amount       bigint unsigned not null,
    created      timestamp not null default current_timestamp,
    nostr        text,
    payment_type tinyint unsigned not null,
    fee          bigint unsigned not null default 0,
    constraint fk_payment_user
        foreign key (user_id) references user (id)
);

