delete from campaign_counters;
alter table campaign_counters drop constraint campaign_counters_pkey;
alter table campaign_counters add column partition smallint not null;
alter table campaign_counters add primary key (campaign_id, partition, phase, state);
insert into campaign_counters (campaign_id, partition, phase, state, count)
    select campaign_id, partition, phase, state, count(*) from campaign_nodes
    where campaign_id in (select id from campaigns)
    group by campaign_id, partition, phase, state;
