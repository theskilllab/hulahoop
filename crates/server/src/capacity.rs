//! Runs share the host. Each run reports what it used, the next run's need is
//! predicted from that history, and a queued run starts only when the runner
//! says there is room for it. A run that does not fit waits with a reason a
//! person can read, and starts by itself once room frees up.

use crate::engine::Engine;
use crate::model::Run;
use anyhow::Result;
use async_graphql::SimpleObject;
use chrono::{DateTime, Utc};
use uuid::Uuid;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;
/// What a run with too little history to go on is assumed to need: a build
/// and an agent harness, with room to spare rather than an optimistic guess.
const DEFAULT_MEMORY: u64 = 3 * GIB;
const DEFAULT_DISK: u64 = 4 * GIB;
const DEFAULT_CPU: f64 = 1.0;
/// A run with no repository works in an empty scratch directory.
const REPO_LESS_DISK: u64 = 512 * MIB;
/// A later turn reuses its task's checkout, so it only adds to it.
const LATER_TURN_DISK: u64 = GIB;
/// How many measured runs a prediction reads, newest first, and how few are
/// too few to mean anything.
const HISTORY: i64 = 20;
const MIN_HISTORY: usize = 3;
/// A run that has waited this long for room stops newer, smaller runs from
/// jumping ahead of it; otherwise a big run could wait forever.
const FAIRNESS_SECS: i64 = 10 * 60;
/// A runner reports its capacity with every claim; an older report says
/// nothing about the host now.
const REPORT_FRESH_SECS: i64 = 30;

/// What a run is expected to take from the host while it runs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Need {
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub cpu_cores: f64,
    /// Which history the numbers came from, for logs and tests.
    #[serde(skip)]
    pub basis: &'static str,
}

/// The room a runner has for one more run, after its own reserve and what
/// its live runs are still expected to take.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub cpu_cores: f64,
    /// The runner's cap on live runs (`HULAHOOP_MAX_RUNS`) and how many it has.
    #[serde(default)]
    pub max_runs: Option<u32>,
    #[serde(default)]
    pub running: Option<u32>,
    /// The host is inside its memory reserve: no new run may start, not even
    /// on an idle runner. Kept apart from `memory_bytes`, which also reads zero
    /// when the runner's live runs have merely reserved all the room.
    #[serde(default)]
    pub pressure: bool,
    /// The room the runner would have with none of its runs live. A run that
    /// needs more than this never fits beside others, so it does not hold the
    /// queue for them; it starts when the runner is idle.
    #[serde(default)]
    pub idle_memory_bytes: Option<u64>,
    #[serde(default)]
    pub idle_disk_bytes: Option<u64>,
}

impl Capacity {
    fn full(&self) -> Option<(u32, u32)> {
        match (self.max_runs, self.running) {
            (Some(max), Some(running)) if running >= max => Some((running, max)),
            _ => None,
        }
    }

    /// Whether `need` fits the room the runner has when nothing of its own
    /// runs; a runner that does not say is taken to have it.
    fn fits_idle(&self, need: &Need) -> bool {
        self.idle_memory_bytes
            .is_none_or(|room| need.memory_bytes <= room)
            && self
                .idle_disk_bytes
                .is_none_or(|room| need.disk_bytes <= room)
    }
}

/// The last capacity a runner sent, and when.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Report {
    capacity: Capacity,
    at: DateTime<Utc>,
}

/// What the Tasks page shows about the host: how many runs are going and
/// waiting, and the room the runner last reported.
#[derive(Debug, Clone, SimpleObject)]
pub struct RunnerCapacity {
    pub running: i64,
    pub waiting: i64,
    pub free_memory_bytes: i64,
    pub free_disk_bytes: i64,
    pub free_cpu_cores: f64,
    pub max_runs: Option<i64>,
    pub reported_at: DateTime<Utc>,
}

/// The value at the given percentile, by the nearest-rank method: the
/// smallest value at least `p` percent of the sample is at or below.
pub fn percentile<T: PartialOrd + Copy>(values: &[T], p: f64) -> Option<T> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// Bytes the way the board writes them: decimal gigabytes, one decimal.
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Whether a run with this need may start now, and if not, why not in words
/// a person reads. A host inside its memory reserve takes nothing new. A
/// runner with nothing live takes the run whatever its need: that is how the
/// loop worked before runs shared the host, so a small host, a nearly full
/// disk, or a prediction bigger than the host's room still makes progress one
/// run at a time instead of never starting anything. Beside live runs,
/// memory, disk and CPU must all fit.
pub fn admit(need: &Need, capacity: &Capacity, live_on_runner: u32) -> Result<(), String> {
    if let Some((running, max)) = capacity.full() {
        return Err(slot_reason(running, max));
    }
    if capacity.pressure {
        return Err(PRESSURE.to_string());
    }
    if live_on_runner == 0 {
        return Ok(());
    }
    if need.memory_bytes > capacity.memory_bytes {
        return Err(format!(
            "Waiting for memory: needs ~{}, {} free",
            gb(need.memory_bytes),
            gb(capacity.memory_bytes)
        ));
    }
    if need.disk_bytes > capacity.disk_bytes {
        return Err(format!(
            "Waiting for disk: needs ~{}, {} free",
            gb(need.disk_bytes),
            gb(capacity.disk_bytes)
        ));
    }
    if need.cpu_cores > capacity.cpu_cores {
        return Err(format!(
            "Waiting for CPU: needs ~{:.1} cores, {:.1} free",
            need.cpu_cores,
            capacity.cpu_cores.max(0.0)
        ));
    }
    Ok(())
}

/// The reason a queued run waits behind an older one that has waited too long.
const HELD_FOR_OLDER: &str = "Waiting for an older run that needs more room";
const HELD_FOR_TASK: &str = "Waiting for this task's other run to finish";
const PRESSURE: &str = "Waiting for memory: the host is short of memory";

fn slot_reason(running: u32, max: u32) -> String {
    format!("Waiting for a free slot: {running} of {max} runs are going")
}

/// What a run is assumed to need when its history cannot be read.
fn default_need(repo_less: bool, later_turn: bool) -> Need {
    Need {
        memory_bytes: DEFAULT_MEMORY,
        disk_bytes: if repo_less {
            REPO_LESS_DISK
        } else if later_turn {
            LATER_TURN_DISK
        } else {
            DEFAULT_DISK
        },
        cpu_cores: DEFAULT_CPU,
        basis: "defaults",
    }
}

/// One measured run: peak memory, average cores, checkout growth.
type Measure = (i64, f64, i64);

impl Engine {
    /// Measured runs, newest first: on `repo` (none means repo-less runs),
    /// by `agent` when one is named. `later_turns_only` keeps the runs that
    /// were not their task's first, whose checkout already existed.
    async fn measured_history(
        &self,
        repo: Option<Uuid>,
        agent: Option<Uuid>,
        later_turns_only: bool,
    ) -> Result<Vec<Measure>> {
        let mut sql = String::from(
            "select r.peak_memory_bytes, r.cpu_cores, r.disk_growth_bytes from runs r
              where r.peak_memory_bytes is not null and r.cpu_cores is not null
                and r.disk_growth_bytes is not null and r.finished_at is not null",
        );
        sql.push_str(if repo.is_some() {
            " and r.repo_id = ?"
        } else {
            " and r.repo_id is null"
        });
        if agent.is_some() {
            sql.push_str(" and r.agent_id = ?");
        }
        if later_turns_only {
            sql.push_str(
                " and exists (select 1 from runs p where p.issue_id = r.issue_id and p.created_at < r.created_at)",
            );
        }
        sql.push_str(" order by r.finished_at desc limit ?");
        let mut query = sqlx::query_as::<_, Measure>(&sql);
        if let Some(repo) = repo {
            query = query.bind(repo);
        }
        if let Some(agent) = agent {
            query = query.bind(agent);
        }
        Ok(query.bind(HISTORY).fetch_all(&self.db.pool).await?)
    }

    /// The first history in `tiers` with enough runs to go on, and its name.
    async fn history_from(
        &self,
        tiers: &[(Option<Uuid>, Option<Uuid>, &'static str)],
        later_turns_only: bool,
    ) -> Result<Option<(Vec<Measure>, &'static str)>> {
        for (repo, agent, basis) in tiers {
            let history = self
                .measured_history(*repo, *agent, later_turns_only)
                .await?;
            if history.len() >= MIN_HISTORY {
                return Ok(Some((history, basis)));
            }
        }
        Ok(None)
    }

    /// Whether the run's task already has its checkout: an earlier run of the
    /// task made it, a task that is not done or cancelled still keeps it, and
    /// no cleanup was queued for it since (a task that moved to another
    /// repository loses its checkout while it stays open).
    async fn checkout_exists(&self, run: &Run) -> Result<bool> {
        if run.repo_id.is_none() {
            return Ok(false);
        }
        let (latest,): (Option<String>,) = sqlx::query_as(
            "select max(created_at) from runs where issue_id = ? and id != ?
                and created_at <= (select created_at from runs where id = ?)",
        )
        .bind(run.issue_id)
        .bind(run.id)
        .bind(run.id)
        .fetch_one(&self.db.pool)
        .await?;
        let Some(latest) = latest else {
            return Ok(false);
        };
        let (cleaned,): (i64,) = sqlx::query_as(
            "select count(*) from runner_jobs where kind = 'cleanup' and issue_id = ? and created_at > ?",
        )
        .bind(run.issue_id)
        .bind(latest)
        .fetch_one(&self.db.pool)
        .await?;
        if cleaned > 0 {
            return Ok(false);
        }
        let issue = self.issue(run.issue_id).await?;
        Ok(!issue.status_enum().is_terminal())
    }

    /// What a run is expected to need: the 90th percentile of what runs like
    /// it used. Like it means the same repository and agent; with fewer than
    /// three of those, the same repository; with fewer still, the defaults.
    pub async fn predict_need(&self, run: &Run) -> Result<Need> {
        let later_turn = self.checkout_exists(run).await?;
        self.predict_for(run.repo_id, run.agent_id, later_turn)
            .await
    }

    /// [`Engine::predict_need`] for a run on `repo` by `agent`; `later_turn`
    /// says the task's checkout already exists.
    async fn predict_for(&self, repo: Option<Uuid>, agent: Uuid, later_turn: bool) -> Result<Need> {
        let tiers: Vec<(Option<Uuid>, Option<Uuid>, &'static str)> = match repo {
            Some(repo) => vec![
                (Some(repo), Some(agent), "repository and agent"),
                (Some(repo), None, "repository"),
            ],
            None => vec![(None, Some(agent), "agent")],
        };
        let found = self.history_from(&tiers, false).await?;
        let (history, basis) = match &found {
            Some((history, basis)) => (history.as_slice(), *basis),
            None => (&[][..], "defaults"),
        };
        let memory: Vec<u64> = history.iter().map(|m| m.0.max(0) as u64).collect();
        let cpu: Vec<f64> = history.iter().map(|m| m.1.max(0.0)).collect();
        let disk: Vec<u64> = history.iter().map(|m| m.2.max(0) as u64).collect();

        let disk_bytes = if repo.is_none() {
            percentile(&disk, 90.0).unwrap_or(REPO_LESS_DISK)
        } else if later_turn {
            // A later turn adds to a checkout that is already there, so only
            // what later turns added says anything about it.
            match self.history_from(&tiers, true).await? {
                Some((later, _)) => {
                    let growth: Vec<u64> = later.iter().map(|m| m.2.max(0) as u64).collect();
                    percentile(&growth, 90.0).unwrap_or(LATER_TURN_DISK)
                }
                None => LATER_TURN_DISK,
            }
        } else {
            percentile(&disk, 90.0).unwrap_or(DEFAULT_DISK)
        };
        Ok(Need {
            // Never zero: a run always takes some memory, so a runner whose
            // live runs reserved all the room fits nothing more beside them.
            memory_bytes: percentile(&memory, 90.0).unwrap_or(DEFAULT_MEMORY).max(1),
            disk_bytes,
            cpu_cores: percentile(&cpu, 90.0).unwrap_or(DEFAULT_CPU),
            basis,
        })
    }

    /// Writes why a queued run waits, only when the reason changed, so a
    /// runner polling every second does not rewrite the row every second.
    async fn set_wait_reason(&self, run_id: Uuid, reason: &str) -> Result<()> {
        sqlx::query(
            "update runs set wait_reason = ? where id = ? and status = 'queued' and wait_reason is not ?",
        )
        .bind(reason)
        .bind(run_id)
        .bind(reason)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// The prediction for `run`, or the defaults when its history cannot be
    /// read: one bad row must not stop every claim.
    async fn predict_or_default(&self, run: &Run) -> Need {
        match self.predict_need(run).await {
            Ok(need) => need,
            Err(error) => {
                tracing::warn!(run_id = %run.id, %error, "could not predict a run's need; assuming the defaults");
                default_need(run.repo_id.is_none(), false)
            }
        }
    }

    /// Keeps what a runner reported, for the Tasks page. A runner with every
    /// slot taken, or a host short of memory, starts nothing, and every
    /// queued run is told so in one write. True when the runner can take a
    /// run now.
    pub async fn report_capacity(&self, capacity: &Capacity) -> Result<bool> {
        if let Ok(mut report) = self.runner_capacity.lock() {
            *report = Some(Report {
                capacity: *capacity,
                at: Utc::now(),
            });
        }
        let reason = match capacity.full() {
            Some((running, max)) => slot_reason(running, max),
            None if capacity.pressure => PRESSURE.to_string(),
            None => return Ok(true),
        };
        sqlx::query(
            "update runs set wait_reason = ? where status = 'queued' and wait_reason is not ?",
        )
        .bind(&reason)
        .bind(&reason)
        .execute(&self.db.pool)
        .await?;
        Ok(false)
    }

    /// Hands a queued run to the runner, with the need it was admitted on so
    /// the runner keeps that much room for it. Without a capacity (an older
    /// runner) the oldest queued run goes, as it always did. With one, queued
    /// runs are tried oldest first and the first that fits goes; every run
    /// passed over is told why it waits. A run that has waited longer than ten
    /// minutes holds the queue, so nothing newer starts before it, as long as
    /// it fits an idle runner at all: one that never fits beside others would
    /// hold it for nothing, and starts once the runner is idle instead.
    pub async fn claim_run(
        &self,
        runner_id: &str,
        capacity: Option<Capacity>,
    ) -> Result<Option<(Run, Need)>> {
        let Some(capacity) = capacity else {
            let candidate: Option<Run> = sqlx::query_as(
                "select * from runs where status = 'queued' order by created_at limit 1",
            )
            .fetch_optional(&self.db.pool)
            .await?;
            let Some(run) = candidate else {
                return Ok(None);
            };
            let need = self.predict_or_default(&run).await;
            return Ok(self.take(run.id, runner_id).await?.map(|run| (run, need)));
        };
        if !self.report_capacity(&capacity).await? {
            return Ok(None);
        }

        let (live_on_runner,): (i64,) =
            sqlx::query_as("select count(*) from runs where status = 'running' and runner_id = ?")
                .bind(runner_id)
                .fetch_one(&self.db.pool)
                .await?;
        // The runner knows its own live runs best; the table is the fallback
        // for a runner that does not say.
        let live_on_runner = capacity.running.unwrap_or(live_on_runner.max(0) as u32);

        let busy: std::collections::HashSet<Uuid> =
            sqlx::query_as::<_, (Uuid,)>("select issue_id from runs where status = 'running'")
                .fetch_all(&self.db.pool)
                .await?
                .into_iter()
                .map(|(issue,)| issue)
                .collect();
        let queued: Vec<Run> =
            sqlx::query_as("select * from runs where status = 'queued' order by created_at")
                .fetch_all(&self.db.pool)
                .await?;
        // Runs of one repository by one agent at the same stage of their task
        // are predicted alike, so a long queue reads each history once.
        let mut predicted: std::collections::HashMap<(Option<Uuid>, Uuid, bool), Need> =
            std::collections::HashMap::new();
        let now = Utc::now();
        let mut held = false;
        for run in queued {
            if held {
                self.set_wait_reason(run.id, HELD_FOR_OLDER).await?;
                continue;
            }
            if busy.contains(&run.issue_id) {
                self.set_wait_reason(run.id, HELD_FOR_TASK).await?;
                continue;
            }
            let later_turn = match self.checkout_exists(&run).await {
                Ok(exists) => exists,
                Err(error) => {
                    tracing::warn!(run_id = %run.id, %error, "could not tell whether a run's checkout exists");
                    false
                }
            };
            let key = (run.repo_id, run.agent_id, later_turn);
            let need = match predicted.get(&key) {
                Some(need) => *need,
                None => {
                    let need = match self
                        .predict_for(run.repo_id, run.agent_id, later_turn)
                        .await
                    {
                        Ok(need) => need,
                        Err(error) => {
                            tracing::warn!(run_id = %run.id, %error, "could not predict a run's need; assuming the defaults");
                            default_need(run.repo_id.is_none(), later_turn)
                        }
                    };
                    predicted.insert(key, need);
                    need
                }
            };
            match admit(&need, &capacity, live_on_runner) {
                Ok(()) => {
                    if let Some(claimed) = self.take(run.id, runner_id).await? {
                        tracing::info!(
                            run_id = %claimed.id,
                            memory = need.memory_bytes,
                            disk = need.disk_bytes,
                            cpu = need.cpu_cores,
                            basis = need.basis,
                            "claimed a run that fits"
                        );
                        return Ok(Some((claimed, need)));
                    }
                }
                Err(reason) => {
                    self.set_wait_reason(run.id, &reason).await?;
                    if (now - run.created_at).num_seconds() > FAIRNESS_SECS
                        && capacity.fits_idle(&need)
                    {
                        held = true;
                    }
                }
            }
        }
        Ok(None)
    }

    /// Claims one queued run atomically: a second claimer finds it taken.
    async fn take(&self, run_id: Uuid, runner_id: &str) -> Result<Option<Run>> {
        let now = Utc::now().to_rfc3339();
        let updated = sqlx::query(
            "update runs set status = 'running', runner_id = ?, claimed_at = ?, started_at = ?, last_heartbeat_at = ?, wait_reason = null where id = ? and status = 'queued'",
        )
        .bind(runner_id)
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .bind(run_id)
        .execute(&self.db.pool)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(None);
        }
        self.run(run_id).await.map(Some)
    }

    /// What the runner last reported, with the runs going and waiting now;
    /// none when no runner reported in the last thirty seconds.
    pub async fn runner_capacity(&self) -> Result<Option<RunnerCapacity>> {
        let report = self.runner_capacity.lock().ok().and_then(|report| *report);
        let Some(report) = report else {
            return Ok(None);
        };
        if (Utc::now() - report.at).num_seconds() > REPORT_FRESH_SECS {
            return Ok(None);
        }
        let (running, waiting): (i64, i64) = sqlx::query_as(
            "select coalesce(sum(status = 'running'), 0), coalesce(sum(status = 'queued'), 0) from runs",
        )
        .fetch_one(&self.db.pool)
        .await?;
        let bytes = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        Ok(Some(RunnerCapacity {
            running,
            waiting,
            free_memory_bytes: bytes(report.capacity.memory_bytes),
            free_disk_bytes: bytes(report.capacity.disk_bytes),
            free_cpu_cores: report.capacity.cpu_cores.max(0.0),
            max_runs: report.capacity.max_runs.map(i64::from),
            reported_at: report.at,
        }))
    }

    /// Records what the runner measured of a finished run. Growth below zero
    /// (a turn that deleted more than it built) is stored as none.
    pub(crate) async fn record_resources(
        &self,
        run_id: Uuid,
        peak_memory_bytes: Option<i64>,
        cpu_cores: Option<f64>,
        disk_growth_bytes: Option<i64>,
    ) -> Result<()> {
        if peak_memory_bytes.is_none() && cpu_cores.is_none() && disk_growth_bytes.is_none() {
            return Ok(());
        }
        sqlx::query(
            "update runs set peak_memory_bytes = coalesce(?, peak_memory_bytes),
                cpu_cores = coalesce(?, cpu_cores),
                disk_growth_bytes = coalesce(?, disk_growth_bytes) where id = ?",
        )
        .bind(peak_memory_bytes.map(|bytes| bytes.max(0)))
        .bind(
            cpu_cores
                .filter(|cores| cores.is_finite())
                .map(|cores| cores.max(0.0)),
        )
        .bind(disk_growth_bytes.map(|bytes| bytes.max(0)))
        .bind(run_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use chrono::Duration;

    #[test]
    fn the_90th_percentile_is_the_nearest_rank() {
        let ten: Vec<u64> = (1..=10).collect();
        assert_eq!(percentile(&ten, 90.0), Some(9));
        assert_eq!(percentile(&[5u64, 1, 3], 90.0), Some(5));
        assert_eq!(percentile(&[2.5f64], 90.0), Some(2.5));
        let twenty: Vec<u64> = (1..=20).rev().collect();
        assert_eq!(percentile(&twenty, 90.0), Some(18));
        assert_eq!(percentile::<u64>(&[], 90.0), None);
    }

    fn need(memory: u64, disk: u64, cpu: f64) -> Need {
        Need {
            memory_bytes: memory,
            disk_bytes: disk,
            cpu_cores: cpu,
            basis: "test",
        }
    }

    fn room(memory: u64, disk: u64, cpu: f64) -> Capacity {
        Capacity {
            memory_bytes: memory,
            disk_bytes: disk,
            cpu_cores: cpu,
            max_runs: Some(4),
            running: Some(0),
            pressure: false,
            idle_memory_bytes: None,
            idle_disk_bytes: None,
        }
    }

    /// [`room`] on a runner that already has one run live.
    fn beside_one(memory: u64, disk: u64, cpu: f64) -> Capacity {
        Capacity {
            running: Some(1),
            ..room(memory, disk, cpu)
        }
    }

    #[test]
    fn a_run_that_fits_is_admitted_and_one_that_does_not_says_why() {
        let gb = 1_000_000_000;
        assert_eq!(
            admit(&need(2 * gb, gb, 1.0), &room(3 * gb, 2 * gb, 2.0), 1),
            Ok(())
        );
        assert_eq!(
            admit(
                &need(2_400_000_000, gb, 1.0),
                &room(1_100_000_000, 9 * gb, 4.0),
                1
            ),
            Err("Waiting for memory: needs ~2.4 GB, 1.1 GB free".to_string())
        );
        assert_eq!(
            admit(&need(gb, 4_300_000_000, 1.0), &room(9 * gb, 2 * gb, 4.0), 1),
            Err("Waiting for disk: needs ~4.3 GB, 2.0 GB free".to_string())
        );
        assert_eq!(
            admit(&need(gb, gb, 1.0), &room(9 * gb, 9 * gb, 0.4), 1),
            Err("Waiting for CPU: needs ~1.0 cores, 0.4 free".to_string())
        );
        let full = Capacity {
            running: Some(4),
            ..room(9 * gb, 9 * gb, 8.0)
        };
        assert_eq!(
            admit(&need(gb, gb, 1.0), &full, 4),
            Err("Waiting for a free slot: 4 of 4 runs are going".to_string())
        );
    }

    /// An idle runner takes the run whatever its need, as the loop did before
    /// runs shared the host: a host smaller than the defaults, a disk whose
    /// reserve leaves nothing, or busy cores never stop the one run. Only a
    /// host short of memory starts nothing.
    #[test]
    fn an_idle_runner_takes_a_run_unless_it_is_short_of_memory() {
        let gb = 1_000_000_000;
        // A 4 GB host: 3 GiB of defaults never fit in what is left after the
        // reserve, and the run starts anyway.
        assert_eq!(
            admit(&need(3 * GIB, 4 * GIB, 1.0), &room(2 * gb, 0, -1.0), 0),
            Ok(())
        );
        let pressed = Capacity {
            pressure: true,
            ..room(0, 9 * gb, 8.0)
        };
        assert_eq!(
            admit(&need(gb, gb, 1.0), &pressed, 0),
            Err("Waiting for memory: the host is short of memory".to_string())
        );
        assert_eq!(
            admit(&need(gb, gb, 1.0), &pressed, 1),
            Err("Waiting for memory: the host is short of memory".to_string())
        );
    }

    const DOC: &str = "# Workflows\n\nTurns: 6\n\n## Backlog\n\nUsers: human\n\nNext: In progress\n\n## In progress\n\nUsers: @Developer\n\nNext: Done\n\n## Blocked\n\nUsers: human\n\nNext: In progress\n\n## Done\n\nUsers: human\n\nNext: none\n";

    struct World {
        directory: std::path::PathBuf,
        engine: Engine,
        workspace: Uuid,
        project: Uuid,
        repo: Uuid,
        developer: Uuid,
        tester: Uuid,
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    /// A repository with two agents, both of which passed preflight on it.
    async fn world(tag: &str) -> World {
        let directory =
            std::env::temp_dir().join(format!("hulahoop-capacity-{tag}-{}", Uuid::now_v7()));
        let db = Db::open(&directory).await.unwrap();
        let workspace = db.seed_local().await.unwrap();
        let (project, repo, developer, tester) = (
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        );
        let now = Utc::now() - Duration::hours(2);
        sqlx::query(
            "insert into projects (id, workspace_id, name, created_at) values (?,?,'Keep',?)",
        )
        .bind(project)
        .bind(workspace)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        sqlx::query("insert into repos (id, project_id, name, remote_url, contract_version, created_at) values (?,?,'repo','/tmp/repo','1',?)")
            .bind(repo).bind(project).bind(now).execute(&db.pool).await.unwrap();
        for (agent, name) in [(developer, "Developer"), (tester, "Tester")] {
            sqlx::query("insert into agents (id, workspace_id, name, adapter, model, created_at, updated_at) values (?,?,?,'pi','model-x',?,?)")
                .bind(agent).bind(workspace).bind(name).bind(now).bind(now).execute(&db.pool).await.unwrap();
            sqlx::query("insert into agent_preflight (agent_id, repo_id, ok, detail, checked_at) values (?,?,1,'ok',?)")
                .bind(agent).bind(repo).bind(now).execute(&db.pool).await.unwrap();
        }
        let engine = Engine::new(db);
        engine.set_workflow(workspace, DOC).await.unwrap();
        World {
            directory,
            engine,
            workspace,
            project,
            repo,
            developer,
            tester,
        }
    }

    /// A task in progress, on the world's repository or on none.
    async fn task(world: &World, number: i64, on_repo: bool) -> Uuid {
        let issue = Uuid::now_v7();
        let now = Utc::now();
        let (project, repo) = if on_repo {
            (Some(world.project), Some(world.repo))
        } else {
            (None, None)
        };
        sqlx::query("insert into issues (id, workspace_id, project_id, repo_id, issue_key, number, title, status, created_at, updated_at) values (?,?,?,?,?,?,'Task','in_progress',?,?)")
            .bind(issue).bind(world.workspace).bind(project).bind(repo)
            .bind(format!("KEP-{number}")).bind(number).bind(now).bind(now)
            .execute(&world.engine.db.pool).await.unwrap();
        issue
    }

    /// A finished run on `issue` that measured what it used.
    async fn measured(
        world: &World,
        issue: Uuid,
        repo: Option<Uuid>,
        agent: Uuid,
        (memory, cpu, disk): Measure,
    ) {
        let at = Utc::now() - Duration::minutes(30);
        sqlx::query("insert into runs (id, issue_id, repo_id, agent_id, attempt, base_bookmark, prompt_revision, artifact_dir, status, model, created_at, finished_at, peak_memory_bytes, cpu_cores, disk_growth_bytes) values (?,?,?,?,1,'main','prompt','/tmp','succeeded','model-x',?,?,?,?,?)")
            .bind(Uuid::now_v7()).bind(issue).bind(repo).bind(agent)
            .bind(crate::db::now_rfc3339()).bind(at).bind(memory).bind(cpu).bind(disk)
            .execute(&world.engine.db.pool).await.unwrap();
        // Distinct creation times keep "the first run of its task" well defined.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    async fn queued(world: &World, issue: Uuid, agent: Uuid) -> Run {
        world
            .engine
            .start_run(issue, Some(agent), None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_run_with_no_history_is_given_the_defaults() {
        let world = world("defaults").await;
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        let need = world.engine.predict_need(&run).await.unwrap();
        assert_eq!(need.basis, "defaults");
        assert_eq!(need.memory_bytes, 3 * GIB);
        assert_eq!(need.disk_bytes, 4 * GIB);
        assert_eq!(need.cpu_cores, 1.0);

        let scratch = task(&world, 2, false).await;
        let run = queued(&world, scratch, world.developer).await;
        let need = world.engine.predict_need(&run).await.unwrap();
        assert_eq!(
            need.disk_bytes,
            512 * MIB,
            "a repo-less run gets a scratch directory"
        );
    }

    #[tokio::test]
    async fn a_prediction_reads_the_closest_history_with_enough_runs() {
        let world = world("tiers").await;
        // Two runs by the developer are too few; with the tester's three the
        // repository has five, and its 90th percentile is the prediction.
        for (n, memory) in [(10, 100), (11, 200)] {
            let issue = task(&world, n, true).await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.developer,
                (memory, 0.5, 10),
            )
            .await;
        }
        for (n, memory) in [(12, 300), (13, 400), (14, 500)] {
            let issue = task(&world, n, true).await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.tester,
                (memory, 2.0, 20),
            )
            .await;
        }
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        let need = world.engine.predict_need(&run).await.unwrap();
        assert_eq!(need.basis, "repository");
        assert_eq!(need.memory_bytes, 500);
        assert_eq!(need.cpu_cores, 2.0);
        assert_eq!(need.disk_bytes, 20);

        // A third developer run makes the developer's own history enough.
        let issue = task(&world, 15, true).await;
        measured(
            &world,
            issue,
            Some(world.repo),
            world.developer,
            (150, 0.25, 5),
        )
        .await;
        let need = world.engine.predict_need(&run).await.unwrap();
        assert_eq!(need.basis, "repository and agent");
        assert_eq!(need.memory_bytes, 200);
        assert_eq!(need.cpu_cores, 0.5);
    }

    #[tokio::test]
    async fn a_later_turn_predicts_only_what_later_turns_added() {
        let world = world("later-turn").await;
        for n in 10..13 {
            let issue = task(&world, n, true).await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.developer,
                (100, 1.0, 9_000),
            )
            .await;
        }
        // The task already ran once, so its checkout exists. No later turn in
        // the history yet: a gigabyte is assumed.
        let issue = task(&world, 1, true).await;
        measured(
            &world,
            issue,
            Some(world.repo),
            world.developer,
            (100, 1.0, 9_000),
        )
        .await;
        let run = queued(&world, issue, world.developer).await;
        assert_eq!(
            world.engine.predict_need(&run).await.unwrap().disk_bytes,
            GIB
        );

        for n in 10..13 {
            let issue = task(&world, n + 10, true).await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.developer,
                (100, 1.0, 9_000),
            )
            .await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.developer,
                (100, 1.0, 70),
            )
            .await;
        }
        assert_eq!(
            world.engine.predict_need(&run).await.unwrap().disk_bytes,
            70
        );
    }

    #[tokio::test]
    async fn two_runs_on_one_repository_both_start_when_there_is_room() {
        let world = world("same-repo").await;
        let first = task(&world, 1, true).await;
        let second = task(&world, 2, true).await;
        let a = queued(&world, first, world.developer).await;
        let b = queued(&world, second, world.developer).await;
        assert_eq!(a.repo_id, b.repo_id);

        let plenty = room(64 * GIB, 500 * GIB, 16.0);
        let (claimed, need) = world
            .engine
            .claim_run("r1", Some(plenty))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.id, a.id);
        assert_eq!(need.basis, "defaults", "the need it was admitted on");
        let busy = Capacity {
            running: Some(1),
            ..plenty
        };
        let claimed = world.engine.claim_run("r1", Some(busy)).await.unwrap();
        assert_eq!(claimed.map(|(run, _)| run.id), Some(b.id));
        assert!(world.engine.runner_capacity().await.unwrap().is_some());
    }

    #[tokio::test]
    async fn a_claim_without_capacity_takes_the_oldest_run_as_before() {
        let world = world("old-runner").await;
        let first = task(&world, 1, true).await;
        let second = task(&world, 2, true).await;
        let a = queued(&world, first, world.developer).await;
        queued(&world, second, world.developer).await;
        let (claimed, need) = world.engine.claim_run("old", None).await.unwrap().unwrap();
        assert_eq!(claimed.id, a.id);
        assert_eq!(
            need.memory_bytes,
            3 * GIB,
            "an old runner still learns the need"
        );
        assert_eq!(claimed.status, "running");
        assert!(
            world.engine.runner_capacity().await.unwrap().is_none(),
            "an old runner reports nothing"
        );
    }

    #[tokio::test]
    async fn a_run_that_does_not_fit_waits_with_its_reason_until_it_does() {
        let world = world("waits").await;
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        let tight = beside_one(1_100_000_000, 500 * GIB, 8.0);
        assert!(world
            .engine
            .claim_run("r1", Some(tight))
            .await
            .unwrap()
            .is_none());
        let waiting = world.engine.run(run.id).await.unwrap();
        assert_eq!(
            waiting.wait_reason.as_deref(),
            Some("Waiting for memory: needs ~3.2 GB, 1.1 GB free")
        );

        let claimed = world
            .engine
            .claim_run("r1", Some(beside_one(8 * GIB, 500 * GIB, 8.0)))
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(claimed.id, run.id);
        assert_eq!(claimed.wait_reason, None, "a claimed run waits for nothing");
    }

    #[tokio::test]
    async fn a_run_that_waited_too_long_is_not_overtaken() {
        let world = world("fairness").await;
        // The repository's runs are big; repo-less runs of the developer are
        // small, so a newer repo-less run would fit where the older one does not.
        for n in 10..13 {
            let issue = task(&world, n, true).await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.developer,
                (6_000_000_000, 1.0, 1_000),
            )
            .await;
            let scratch = task(&world, n + 10, false).await;
            measured(
                &world,
                scratch,
                None,
                world.developer,
                (100_000_000, 0.1, 1_000),
            )
            .await;
        }
        let big = task(&world, 1, true).await;
        let small = task(&world, 2, false).await;
        let older = queued(&world, big, world.developer).await;
        let newer = queued(&world, small, world.developer).await;
        // Beside one live run there is room for the small run only; an idle
        // runner would have room for the big one too.
        let half = Capacity {
            idle_memory_bytes: Some(8 * GIB),
            idle_disk_bytes: Some(500 * GIB),
            ..beside_one(3 * GIB, 500 * GIB, 8.0)
        };

        // A fresh wait lets the small run go first.
        let claimed = world.engine.claim_run("r1", Some(half)).await.unwrap();
        assert_eq!(claimed.map(|(run, _)| run.id), Some(newer.id));
        sqlx::query("update runs set status = 'queued', runner_id = null where id = ?")
            .bind(newer.id)
            .execute(&world.engine.db.pool)
            .await
            .unwrap();

        // Ten minutes on, the big run holds the queue.
        let long_ago = Utc::now() - Duration::minutes(11);
        sqlx::query("update runs set created_at = ? where id = ?")
            .bind(long_ago)
            .bind(older.id)
            .execute(&world.engine.db.pool)
            .await
            .unwrap();
        assert!(world
            .engine
            .claim_run("r1", Some(half))
            .await
            .unwrap()
            .is_none());
        let held = world.engine.run(newer.id).await.unwrap();
        assert_eq!(
            held.wait_reason.as_deref(),
            Some("Waiting for an older run that needs more room")
        );
        let older = world.engine.run(older.id).await.unwrap();
        assert!(older
            .wait_reason
            .unwrap()
            .starts_with("Waiting for memory: needs ~6.0 GB"));

        // A run bigger than an idle runner's room never fits beside another,
        // so it holds nothing back; it starts once the runner is idle.
        let small_host = Capacity {
            idle_memory_bytes: Some(5 * GIB),
            ..half
        };
        let claimed = world
            .engine
            .claim_run("r1", Some(small_host))
            .await
            .unwrap();
        assert_eq!(claimed.map(|(run, _)| run.id), Some(newer.id));
        let idle = Capacity {
            running: Some(0),
            ..small_host
        };
        let claimed = world.engine.claim_run("r2", Some(idle)).await.unwrap();
        assert_eq!(claimed.map(|(run, _)| run.id), Some(older.id));
    }

    /// An idle runner takes the oldest run even when its prediction is bigger
    /// than the host's room, so a small host still works one run at a time;
    /// a host short of memory takes nothing.
    #[tokio::test]
    async fn an_idle_runner_never_stalls_on_a_big_prediction() {
        let world = world("idle").await;
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        let pressed = Capacity {
            pressure: true,
            ..room(0, 0, 2.0)
        };
        assert!(world
            .engine
            .claim_run("r1", Some(pressed))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            world
                .engine
                .run(run.id)
                .await
                .unwrap()
                .wait_reason
                .as_deref(),
            Some("Waiting for memory: the host is short of memory")
        );
        let small = room(GIB, 0, 1.0);
        let (claimed, need) = world
            .engine
            .claim_run("r1", Some(small))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.id, run.id);
        assert!(need.memory_bytes > small.memory_bytes);
    }

    /// A runner with every slot taken claims nothing, and every queued run
    /// says why in one write; its report still reaches the Tasks page.
    #[tokio::test]
    async fn a_full_runner_takes_nothing_and_says_so() {
        let world = world("full").await;
        let first = task(&world, 1, true).await;
        let second = task(&world, 2, true).await;
        let a = queued(&world, first, world.developer).await;
        let b = queued(&world, second, world.developer).await;
        let full = Capacity {
            running: Some(4),
            ..room(64 * GIB, 500 * GIB, 16.0)
        };
        assert!(!world.engine.report_capacity(&full).await.unwrap());
        assert!(world
            .engine
            .claim_run("r1", Some(full))
            .await
            .unwrap()
            .is_none());
        for run in [a.id, b.id] {
            assert_eq!(
                world.engine.run(run).await.unwrap().wait_reason.as_deref(),
                Some("Waiting for a free slot: 4 of 4 runs are going")
            );
        }
        let report = world.engine.runner_capacity().await.unwrap().unwrap();
        assert_eq!(report.max_runs, Some(4));
    }

    /// A history that cannot be read costs the run its prediction, not the
    /// claim: the run is admitted on the defaults.
    #[tokio::test]
    async fn a_prediction_that_fails_falls_back_to_the_defaults() {
        let world = world("broken-history").await;
        for n in 10..13 {
            let issue = task(&world, n, true).await;
            measured(
                &world,
                issue,
                Some(world.repo),
                world.developer,
                (1, 1.0, 1),
            )
            .await;
        }
        // A row no prediction can read.
        sqlx::query("update runs set peak_memory_bytes = 'a lot' where repo_id = ?")
            .bind(world.repo)
            .execute(&world.engine.db.pool)
            .await
            .unwrap();
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        assert!(world.engine.predict_need(&run).await.is_err());
        let (claimed, need) = world
            .engine
            .claim_run("r1", Some(beside_one(64 * GIB, 500 * GIB, 16.0)))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.id, run.id);
        assert_eq!(need.basis, "defaults");
        assert_eq!(need.memory_bytes, 3 * GIB);
    }

    /// A task whose checkout was cleaned up since its last run (it moved to
    /// another repository) makes a fresh one, so its next run is predicted
    /// the whole checkout.
    #[tokio::test]
    async fn a_cleaned_up_checkout_is_predicted_in_full() {
        let world = world("cleaned").await;
        let issue = task(&world, 1, true).await;
        measured(
            &world,
            issue,
            Some(world.repo),
            world.developer,
            (1, 1.0, 1),
        )
        .await;
        let run = queued(&world, issue, world.developer).await;
        assert_eq!(
            world.engine.predict_need(&run).await.unwrap().disk_bytes,
            GIB
        );
        sqlx::query("insert into runner_jobs (id, repo_id, issue_id, kind, payload, status, created_at) values (?,?,?,'cleanup','{}','succeeded',?)")
            .bind(Uuid::now_v7()).bind(world.repo).bind(issue).bind(crate::db::now_rfc3339())
            .execute(&world.engine.db.pool).await.unwrap();
        assert_eq!(
            world.engine.predict_need(&run).await.unwrap().disk_bytes,
            4 * GIB
        );
    }

    /// A start the server refuses spends no turn: the task keeps every turn
    /// its workflow allows for the run that does start.
    #[tokio::test]
    async fn a_refused_start_does_not_spend_a_turn() {
        let world = world("refused").await;
        let issue = task(&world, 1, true).await;
        sqlx::query("delete from agent_preflight")
            .execute(&world.engine.db.pool)
            .await
            .unwrap();
        let task_row = world.engine.issue(issue).await.unwrap();
        assert!(world
            .engine
            .dispatch_if_agent_owns(&task_row)
            .await
            .is_err());
        let after = world.engine.issue(issue).await.unwrap();
        assert_eq!(after.turns_used, 0);

        sqlx::query("insert into agent_preflight (agent_id, repo_id, ok, detail, checked_at) values (?,?,1,'ok',?)")
            .bind(world.developer).bind(world.repo).bind(Utc::now())
            .execute(&world.engine.db.pool).await.unwrap();
        world.engine.dispatch_if_agent_owns(&after).await.unwrap();
        assert_eq!(world.engine.issue(issue).await.unwrap().turns_used, 1);
        // The turn was spent with the run, so a run that fails at once gets
        // it back however soon it fails.
        world.engine.refund_turn(issue, world.developer).await;
        assert_eq!(world.engine.issue(issue).await.unwrap().turns_used, 0);
    }

    /// The heartbeat's reply says what the server thinks of the run, so a
    /// runner stops a run that was cancelled; an exited agent's group is
    /// forgotten, so a cancel never signals a number reused since.
    #[tokio::test]
    async fn a_heartbeat_answers_with_the_run_status() {
        let world = world("heartbeat").await;
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        world.engine.claim_run("r1", None).await.unwrap().unwrap();
        let beat = world
            .engine
            .heartbeat(run.id, Some(4_242_424), Some(4_242_424))
            .await
            .unwrap();
        assert_eq!(beat.as_deref(), Some("running"));
        assert_eq!(
            world.engine.run(run.id).await.unwrap().pgid,
            Some(4_242_424)
        );
        world.engine.heartbeat(run.id, None, None).await.unwrap();
        assert_eq!(
            world.engine.run(run.id).await.unwrap().pgid,
            Some(4_242_424)
        );
        world
            .engine
            .heartbeat(run.id, Some(0), Some(0))
            .await
            .unwrap();
        let cleared = world.engine.run(run.id).await.unwrap();
        assert_eq!((cleared.pid, cleared.pgid), (None, None));
        world.engine.cancel_run(run.id).await.unwrap();
        let beat = world.engine.heartbeat(run.id, None, None).await.unwrap();
        assert_eq!(beat.as_deref(), Some("cancelled"));
        assert_eq!(
            world
                .engine
                .heartbeat(Uuid::now_v7(), None, None)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn a_finished_run_keeps_what_it_measured() {
        let world = world("measured").await;
        let issue = task(&world, 1, true).await;
        let run = queued(&world, issue, world.developer).await;
        let completion: crate::engine::RunCompletion = serde_json::from_value(serde_json::json!({
            "status": "failed",
            "error": "boom",
            "peakMemoryBytes": 1_500_000_000i64,
            "cpuCores": 1.5,
            "diskGrowthBytes": -20,
        }))
        .unwrap();
        world.engine.complete_run(run.id, completion).await.unwrap();
        let run = world.engine.run(run.id).await.unwrap();
        assert_eq!(run.peak_memory_bytes, Some(1_500_000_000));
        assert_eq!(run.cpu_cores, Some(1.5));
        assert_eq!(
            run.disk_growth_bytes,
            Some(0),
            "a shrinking checkout grew by nothing"
        );
    }
}
