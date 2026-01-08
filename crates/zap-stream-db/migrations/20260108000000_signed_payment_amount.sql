-- Change payment.amount from unsigned to signed to support negative amounts
alter table payment modify column amount bigint not null;
