-- Add audit table for logging admin actions
create table audit_log
(
    id          integer unsigned not null auto_increment primary key,
    admin_id    integer unsigned not null,
    action      varchar(255) not null,
    target_type varchar(100),
    target_id   varchar(100),
    message     text not null,
    metadata    json,
    created     timestamp not null default current_timestamp,
    
    constraint fk_audit_log_admin
        foreign key (admin_id) references user (id)
);

-- Add index for querying audit logs by admin
create index ix_audit_log_admin_id on audit_log (admin_id);

-- Add index for querying audit logs by action
create index ix_audit_log_action on audit_log (action);

-- Add index for querying audit logs by target
create index ix_audit_log_target on audit_log (target_type, target_id);

-- Add index for querying audit logs by date
create index ix_audit_log_created on audit_log (created);
