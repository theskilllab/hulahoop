-- One reply per Lumi job, including retries after a runner disconnects.
alter table lumi_messages add column job_id blob references runner_jobs(id);
create unique index lumi_messages_job_reply on lumi_messages(job_id) where job_id is not null;
create index runner_jobs_lumi_lease on runner_jobs(kind, status, claimed_at);
