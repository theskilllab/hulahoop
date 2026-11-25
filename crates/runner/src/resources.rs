//! What a run takes from the host, and how much room the host has left.
//!
//! The runner samples each run's process group on every heartbeat: the memory
//! all of its processes hold together, and the CPU time they spent. When the
//! run is over it reports the peak memory, the average cores it kept busy and
//! how much its checkout grew, and the server predicts the next run's need
//! from those. Before each claim the runner works out the room it has for one
//! more run, keeping a reserve for the host and what its live runs are still
//! expected to take.
//!
//! Linux reads `/proc`; macOS asks `ps`, `vm_stat`, `sysctl` and `df`. The
//! parsers are plain functions over text, so they are tested on fixtures.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

const GIB: u64 = 1 << 30;

/// What the server expects a run to take while it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Need {
    pub memory_bytes: u64,
    pub disk_bytes: u64,
    pub cpu_cores: f64,
}

impl Need {
    /// What an older server that predicts nothing is assumed to mean: the
    /// same defaults the server uses for a run with no history.
    pub const DEFAULT: Need = Need {
        memory_bytes: 3 * GIB,
        disk_bytes: 4 * GIB,
        cpu_cores: 1.0,
    };

    /// The `need` of a claim envelope; the defaults when the server sent none.
    pub fn from_envelope(need: &Value) -> Need {
        if !need.is_object() {
            return Need::DEFAULT;
        }
        Need {
            memory_bytes: need["memoryBytes"]
                .as_u64()
                .unwrap_or(Need::DEFAULT.memory_bytes),
            disk_bytes: need["diskBytes"]
                .as_u64()
                .unwrap_or(Need::DEFAULT.disk_bytes),
            cpu_cores: need["cpuCores"].as_f64().unwrap_or(Need::DEFAULT.cpu_cores),
        }
    }
}

/// One process of a run's group, as one sample saw it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProcessUse {
    pub pid: i64,
    /// When the process started, in clock ticks since boot; zero when `ps`
    /// cannot say. A pid the system reused names another process.
    pub start: u64,
    pub ppid: i64,
    pub pgrp: i64,
    pub rss_bytes: u64,
    /// CPU seconds the process spent itself.
    pub own_cpu_secs: f64,
    /// CPU seconds of the children it has reaped; `ps` cannot tell, so zero there.
    pub children_cpu_secs: f64,
}

/// One line of `/proc/<pid>/stat`. The command name sits in parentheses and
/// may itself hold spaces and parentheses, so the fields are counted from the
/// last `)`.
pub fn parse_proc_stat(line: &str, page_size: u64, ticks_per_sec: f64) -> Option<ProcessUse> {
    let pid: i64 = line.split_whitespace().next()?.parse().ok()?;
    let rest = &line[line.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // After the name: state(3) ppid(4) pgrp(5) ... utime(14) stime(15)
    // cutime(16) cstime(17) ... starttime(22) ... rss(24), counted from 1 for
    // the pid.
    let field = |n: usize| -> Option<i64> { fields.get(n - 3)?.parse().ok() };
    let ticks = |a: usize, b: usize| -> Option<f64> {
        Some((field(a)?.max(0) + field(b)?.max(0)) as f64 / ticks_per_sec)
    };
    Some(ProcessUse {
        pid,
        start: field(22)?.max(0) as u64,
        ppid: field(4)?,
        pgrp: field(5)?,
        rss_bytes: (field(24)?.max(0) as u64).saturating_mul(page_size),
        own_cpu_secs: ticks(14, 15)?,
        children_cpu_secs: ticks(16, 17)?,
    })
}

/// `MemTotal` and `MemAvailable` from `/proc/meminfo`, in bytes.
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next();
        let kib = parts.next().and_then(|value| value.parse::<u64>().ok());
        match key {
            Some("MemTotal:") => total = kib.map(|k| k * 1024),
            Some("MemAvailable:") => available = kib.map(|k| k * 1024),
            _ => {}
        }
    }
    Some((total?, available?))
}

/// Free plus inactive pages from macOS `vm_stat`, in bytes: what the kernel
/// can hand out without swapping much.
pub fn parse_vm_stat(text: &str) -> Option<u64> {
    let page_size: u64 = text
        .lines()
        .next()?
        .split("page size of")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |name: &str| -> Option<u64> {
        text.lines()
            .find(|line| line.trim_start().starts_with(name))?
            .split(':')
            .nth(1)?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    };
    Some((pages("Pages free")? + pages("Pages inactive")?) * page_size)
}

/// `ps`'s cumulative CPU time: `[[dd-]hh:]mm:ss[.ss]`, in seconds.
pub fn parse_ps_time(text: &str) -> Option<f64> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>().ok()?, clock),
        None => (0.0, text),
    };
    let mut seconds = 0.0;
    for part in clock.split(':') {
        seconds = seconds * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(days * 86_400.0 + seconds)
}

/// The output of `ps -A -o pid=,ppid=,pgid=,rss=,time=`; `rss` is in KiB.
pub fn parse_ps(text: &str) -> Vec<ProcessUse> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 5 {
                return None;
            }
            Some(ProcessUse {
                pid: fields[0].parse().ok()?,
                start: 0,
                ppid: fields[1].parse().ok()?,
                pgrp: fields[2].parse().ok()?,
                rss_bytes: fields[3].parse::<u64>().ok()?.saturating_mul(1024),
                own_cpu_secs: parse_ps_time(fields[4])?,
                children_cpu_secs: 0.0,
            })
        })
        .collect()
}

/// Total and available bytes of the filesystem `df -Pk` describes. The
/// device and the mount point may hold spaces, so the numbers are found as
/// the three in a row before the capacity percentage.
pub fn parse_df(text: &str) -> Option<(u64, u64)> {
    let line = text.lines().nth(1)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    let at = fields.iter().position(|field| {
        field.ends_with('%') && field[..field.len() - 1].parse::<u64>().is_ok()
    })?;
    if at < 3 {
        return None;
    }
    let number = |index: usize| fields[index].parse::<u64>().ok();
    let total = number(at - 3)?;
    let available = number(at - 1)?;
    Some((total * 1024, available * 1024))
}

fn getconf(name: &str) -> Option<u64> {
    let output = Command::new("getconf").arg(name).output().ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn page_size() -> u64 {
    static PAGE: OnceLock<u64> = OnceLock::new();
    *PAGE.get_or_init(|| getconf("PAGESIZE").filter(|&size| size > 0).unwrap_or(4096))
}

fn ticks_per_sec() -> f64 {
    static TICKS: OnceLock<f64> = OnceLock::new();
    *TICKS.get_or_init(|| {
        getconf("CLK_TCK")
            .filter(|&ticks| ticks > 0)
            .map(|ticks| ticks as f64)
            .unwrap_or(100.0)
    })
}

/// Every process in the group `pgid`, now.
pub fn sample_group(pgid: i64) -> Vec<ProcessUse> {
    if pgid <= 0 {
        return Vec::new();
    }
    if let Ok(entries) = std::fs::read_dir("/proc") {
        let (page, ticks) = (page_size(), ticks_per_sec());
        let found: Vec<ProcessUse> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
            })
            .filter_map(|entry| std::fs::read_to_string(entry.path().join("stat")).ok())
            .filter_map(|line| parse_proc_stat(&line, page, ticks))
            .filter(|process| process.pgrp == pgid)
            .collect();
        if !found.is_empty() || Path::new("/proc/self/stat").exists() {
            return found;
        }
    }
    let Ok(output) = Command::new("ps")
        .args(["-A", "-o", "pid=,ppid=,pgid=,rss=,time="])
        .output()
    else {
        return Vec::new();
    };
    parse_ps(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .filter(|process| process.pgrp == pgid)
        .collect()
}

/// CPU seconds a process group spent, as far as samples can tell.
///
/// A process that exits between samples is gone from the next one. If an
/// ancestor of it is still in the group, that ancestor (or a parent it
/// reaped in turn) reaped it, and its time now shows in the ancestor's
/// children time, so it is dropped to avoid counting it twice; otherwise the
/// last time it was seen with is kept. A process is its pid and its start
/// time, so a pid the system handed out again starts from nothing.
#[derive(Debug, Default)]
pub struct CpuTracker {
    /// pid → what the last sample saw of it.
    live: HashMap<i64, Seen>,
    gone: f64,
}

#[derive(Debug, Clone, Copy)]
struct Seen {
    start: u64,
    ppid: i64,
    /// The most CPU seconds it was seen with.
    seconds: f64,
}

impl CpuTracker {
    pub fn observe(&mut self, processes: &[ProcessUse]) {
        let now: HashMap<i64, &ProcessUse> = processes.iter().map(|p| (p.pid, p)).collect();
        let still = |pid: i64, start: u64| now.get(&pid).is_some_and(|p| p.start == start);
        for (pid, seen) in &self.live {
            if still(*pid, seen.start) {
                continue;
            }
            // Up the tree as the last sample knew it, through ancestors that
            // are gone too, to one still here: that one has the time.
            let mut ancestor = seen.ppid;
            let mut reaped = false;
            for _ in 0..self.live.len() {
                if let Some(parent) = self.live.get(&ancestor) {
                    if still(ancestor, parent.start) {
                        reaped = true;
                        break;
                    }
                    ancestor = parent.ppid;
                } else {
                    break;
                }
            }
            if !reaped {
                self.gone += seen.seconds;
            }
        }
        let mut live = HashMap::with_capacity(processes.len());
        for process in processes {
            let now = process.own_cpu_secs + process.children_cpu_secs;
            let before = self
                .live
                .get(&process.pid)
                .filter(|seen| seen.start == process.start)
                .map(|seen| seen.seconds)
                .unwrap_or(0.0);
            live.insert(
                process.pid,
                Seen {
                    start: process.start,
                    ppid: process.ppid,
                    seconds: now.max(before),
                },
            );
        }
        self.live = live;
    }

    pub fn seconds(&self) -> f64 {
        self.gone + self.live.values().map(|seen| seen.seconds).sum::<f64>()
    }
}

#[derive(Debug, Default)]
struct Usage {
    peak_rss: u64,
    cpu: CpuTracker,
    first: Option<Instant>,
    last: Option<Instant>,
    disk_before: Option<u64>,
    disk_after: Option<u64>,
    /// The checkout the run grows, while it runs.
    disk_path: Option<PathBuf>,
}

/// One live run's need and what it has used so far, shared between the run
/// and the loop that decides whether another run fits beside it.
#[derive(Debug)]
pub struct Meter {
    pub need: Need,
    rss: AtomicU64,
    /// How much the run's checkout has grown so far, as last walked.
    grown: AtomicU64,
    usage: Mutex<Usage>,
}

/// What one live run holds of the host now, and what it is expected to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveUse {
    pub need: Need,
    pub rss: u64,
    pub grown: u64,
}

impl Meter {
    pub fn new(need: Need) -> Arc<Meter> {
        Arc::new(Meter {
            need,
            rss: AtomicU64::new(0),
            grown: AtomicU64::new(0),
            usage: Mutex::new(Usage::default()),
        })
    }

    fn usage(&self) -> std::sync::MutexGuard<'_, Usage> {
        self.usage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Samples the run's process group and folds it into the totals.
    pub fn sample(&self, pgid: i64) {
        let processes = sample_group(pgid);
        self.observe(&processes, Instant::now());
    }

    pub fn observe(&self, processes: &[ProcessUse], at: Instant) {
        let rss: u64 = processes.iter().map(|p| p.rss_bytes).sum();
        self.rss.store(rss, Ordering::Relaxed);
        let mut usage = self.usage();
        usage.peak_rss = usage.peak_rss.max(rss);
        usage.cpu.observe(processes);
        usage.first.get_or_insert(at);
        usage.last = Some(at);
    }

    /// The memory the run's processes hold now.
    pub fn rss(&self) -> u64 {
        self.rss.load(Ordering::Relaxed)
    }

    /// What the run holds now beside what it is expected to.
    pub fn live_use(&self) -> LiveUse {
        LiveUse {
            need: self.need,
            rss: self.rss(),
            grown: self.grown.load(Ordering::Relaxed),
        }
    }

    /// The agent is gone; what it holds is nothing.
    pub fn stopped(&self) {
        self.rss.store(0, Ordering::Relaxed);
    }

    /// The run grows `path`, which held `before` bytes when it started; the
    /// growth is walked now and then while the run goes (`sample_disk`).
    pub fn watch_disk(&self, path: &Path, before: u64) {
        let mut usage = self.usage();
        usage.disk_before = Some(before);
        usage.disk_path = Some(path.to_path_buf());
    }

    /// Walks the watched checkout. Slow on a big one, so it runs on a thread
    /// of its own and only every minute or so.
    pub fn sample_disk(&self) {
        let (path, before) = {
            let usage = self.usage();
            match (&usage.disk_path, usage.disk_before) {
                (Some(path), Some(before)) => (path.clone(), before),
                _ => return,
            }
        };
        let grown = dir_size(&path).saturating_sub(before);
        self.grown.store(grown, Ordering::Relaxed);
    }

    pub fn disk_after(&self, bytes: u64) {
        self.usage().disk_after = Some(bytes);
    }

    /// The measures a completion carries; each is left out when unknown.
    pub fn report(&self) -> Value {
        let usage = self.usage();
        let mut report = serde_json::Map::new();
        if usage.first.is_some() {
            report.insert("peakMemoryBytes".into(), json!(usage.peak_rss));
        }
        if let (Some(first), Some(last)) = (usage.first, usage.last) {
            let wall = last.duration_since(first).as_secs_f64();
            if wall >= 0.01 {
                report.insert("cpuCores".into(), json!(usage.cpu.seconds() / wall));
            }
        }
        if let (Some(before), Some(after)) = (usage.disk_before, usage.disk_after) {
            report.insert(
                "diskGrowthBytes".into(),
                json!(after.saturating_sub(before)),
            );
        }
        Value::Object(report)
    }

    /// A completion body with this run's measures added.
    pub fn attach(&self, mut body: Value) -> Value {
        if let (Some(body), Value::Object(report)) = (body.as_object_mut(), self.report()) {
            body.extend(report);
        }
        body
    }
}

/// What the host has, as probed now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Host {
    pub memory_total: u64,
    pub memory_available: u64,
    pub disk_total: u64,
    pub disk_available: u64,
    pub cores: f64,
}

fn memory() -> Option<(u64, u64)> {
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        return parse_meminfo(&text);
    }
    let vm_stat = Command::new("vm_stat").output().ok()?;
    let available = parse_vm_stat(&String::from_utf8_lossy(&vm_stat.stdout))?;
    let total = Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8_lossy(&output.stdout).trim().parse().ok())?;
    Some((total, available))
}

fn disk(dir: &Path) -> Option<(u64, u64)> {
    let output = Command::new("df").arg("-Pk").arg(dir).output().ok()?;
    parse_df(&String::from_utf8_lossy(&output.stdout))
}

/// Memory, disk under `data_dir`, and cores; none when the host cannot say.
pub fn probe_host(data_dir: &Path) -> Option<Host> {
    let (memory_total, memory_available) = memory()?;
    let (disk_total, disk_available) = disk(data_dir)?;
    let cores = std::thread::available_parallelism()
        .map(|n| n.get() as f64)
        .unwrap_or(1.0);
    Some(Host {
        memory_total,
        memory_available,
        disk_total,
        disk_available,
        cores,
    })
}

/// What the host keeps for itself: 15% of memory (at least 1 GiB) and 10%
/// of the disk, between 5 and 10 GiB. A run never gets these. The disk share
/// is capped because 10% of a large disk is more than a host needs to keep
/// working, and on a mostly full one it left no room for a second run.
pub fn memory_reserve(host: &Host) -> u64 {
    (host.memory_total / 100 * 15).max(GIB)
}

pub fn disk_reserve(host: &Host) -> u64 {
    (host.disk_total / 10).clamp(5 * GIB, 10 * GIB)
}

/// The room for one more run, as the claim request carries it.
///
/// Memory: what is available, less the reserve, less what each live run is
/// still expected to grow into (its need beyond what it holds now). A host
/// already inside its reserve is under pressure, and says so: it takes no
/// new run at all. Disk: what is free, less the reserve, less what each live
/// run is still expected to write (its need beyond what its checkout grew
/// already, which `df` counts as used). Cores: the logical cores less each
/// live run's need. The idle room is what the runner would have with none of
/// its runs live, so the server can tell a run that fits only an idle runner.
pub fn capacity(host: &Host, live: &[LiveUse], max_runs: usize) -> Value {
    let memory_reserve = memory_reserve(host);
    let disk_reserve = disk_reserve(host);
    let memory = host
        .memory_available
        .saturating_sub(memory_reserve)
        .saturating_sub(
            live.iter()
                .map(|run| run.need.memory_bytes.saturating_sub(run.rss))
                .sum(),
        );
    let disk = host
        .disk_available
        .saturating_sub(disk_reserve)
        .saturating_sub(
            live.iter()
                .map(|run| run.need.disk_bytes.saturating_sub(run.grown))
                .sum(),
        );
    let cpu = host.cores - live.iter().map(|run| run.need.cpu_cores).sum::<f64>();
    let idle_memory = host
        .memory_available
        .saturating_add(live.iter().map(|run| run.rss).sum())
        .saturating_sub(memory_reserve);
    let idle_disk = host
        .disk_available
        .saturating_add(live.iter().map(|run| run.grown).sum())
        .saturating_sub(disk_reserve);
    json!({
        "memoryBytes": memory,
        "diskBytes": disk,
        "cpuCores": cpu,
        "maxRuns": max_runs,
        "running": live.len(),
        "pressure": host.memory_available < memory_reserve,
        "idleMemoryBytes": idle_memory,
        "idleDiskBytes": idle_disk,
    })
}

/// The size of everything under `path`, symlinks counted as themselves and
/// never followed; zero for a path that is not there.
pub fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        // `DirEntry::metadata` does not follow a symlink, so a link out of the
        // checkout costs its own few bytes, not its target's size.
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proc_stat_line_names_its_group_cpu_and_memory() {
        // A command name with a space and a parenthesis in it.
        let line = "4242 (tokio (rt) 1) S 4200 4100 4100 0 -1 4194560 1200 0 0 0 250 50 30 20 20 0 8 0 123456 987654321 1000 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 3 0 0 0 0 0";
        let process = parse_proc_stat(line, 4096, 100.0).unwrap();
        assert_eq!(process.pid, 4242);
        assert_eq!(process.start, 123456);
        assert_eq!(process.ppid, 4200);
        assert_eq!(process.pgrp, 4100);
        assert_eq!(process.rss_bytes, 1000 * 4096);
        assert_eq!(process.own_cpu_secs, 3.0);
        assert_eq!(process.children_cpu_secs, 0.5);
        assert_eq!(parse_proc_stat("garbage", 4096, 100.0), None);
    }

    #[test]
    fn meminfo_gives_total_and_available() {
        let text = "MemTotal:       16318412 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\nBuffers:          100 kB\n";
        assert_eq!(
            parse_meminfo(text),
            Some((16_318_412 * 1024, 8_000_000 * 1024))
        );
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn vm_stat_counts_free_and_inactive_pages() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               10.\nPages active:                            500.\nPages inactive:                            5.\nPages speculative:                         7.\n";
        assert_eq!(parse_vm_stat(text), Some(15 * 16384));
    }

    #[test]
    fn ps_output_reads_on_linux_and_macos() {
        assert_eq!(parse_ps_time("00:00:01"), Some(1.0));
        assert_eq!(parse_ps_time("1-02:03:04"), Some(86_400.0 + 7_384.0));
        assert_eq!(parse_ps_time("0:01.50"), Some(1.5));
        assert_eq!(parse_ps_time("12:34"), Some(754.0));
        let text =
            "    1     0     1  1200 00:00:05\n  300     1   300  2048   0:02.50\nbroken line\n";
        let processes = parse_ps(text);
        assert_eq!(processes.len(), 2);
        assert_eq!(processes[1].pid, 300);
        assert_eq!(processes[1].pgrp, 300);
        assert_eq!(processes[1].rss_bytes, 2048 * 1024);
        assert_eq!(processes[1].own_cpu_secs, 2.5);
    }

    #[test]
    fn df_output_gives_total_and_available() {
        let text = "Filesystem     1024-blocks      Used Available Capacity Mounted on\n/dev/sda1        199820316 173738436  15885416      92% /\n";
        assert_eq!(
            parse_df(text),
            Some((199_820_316 * 1024, 15_885_416 * 1024))
        );
        let spaced = "Filesystem 1024-blocks Used Available Capacity Mounted on\nmap auto home 100 40 60 40% /System/Volumes/Data/home dir\n";
        assert_eq!(parse_df(spaced), Some((100 * 1024, 60 * 1024)));
        assert_eq!(parse_df("Filesystem\n"), None);
    }

    fn process(pid: i64, ppid: i64, own: f64, children: f64) -> ProcessUse {
        ProcessUse {
            pid,
            start: 1,
            ppid,
            pgrp: 1,
            rss_bytes: 100,
            own_cpu_secs: own,
            children_cpu_secs: children,
        }
    }

    #[test]
    fn a_reaped_child_is_counted_once_and_an_orphan_is_kept() {
        let mut cpu = CpuTracker::default();
        cpu.observe(&[process(1, 0, 1.0, 0.0), process(2, 1, 4.0, 0.0)]);
        assert_eq!(cpu.seconds(), 5.0);
        // The parent reaped the child: its time moved into the parent's.
        cpu.observe(&[process(1, 0, 1.5, 5.0), process(3, 99, 2.0, 0.0)]);
        assert_eq!(cpu.seconds(), 8.5);
        // The orphan's parent is not in the group, so its time is kept.
        cpu.observe(&[process(1, 0, 2.0, 5.0)]);
        assert_eq!(cpu.seconds(), 9.0);
    }

    /// A child and its parent that exit between two samples: the grandparent
    /// reaped the parent, which had reaped the child, so both are in the
    /// grandparent's children time and neither counts again.
    #[test]
    fn a_reaped_subtree_is_counted_once() {
        let mut cpu = CpuTracker::default();
        cpu.observe(&[
            process(1, 0, 1.0, 0.0),
            process(2, 1, 2.0, 0.0),
            process(3, 2, 4.0, 0.0),
        ]);
        assert_eq!(cpu.seconds(), 7.0);
        cpu.observe(&[process(1, 0, 1.0, 6.5)]);
        assert_eq!(cpu.seconds(), 7.5);
    }

    /// A pid the system handed to a new process is a new process: it does
    /// not inherit the old one's time, and the old one's time is kept.
    #[test]
    fn a_reused_pid_starts_from_nothing() {
        let mut cpu = CpuTracker::default();
        cpu.observe(&[process(1, 0, 1.0, 0.0), process(2, 99, 9.0, 0.0)]);
        let reused = ProcessUse {
            start: 2,
            ..process(2, 99, 0.5, 0.0)
        };
        cpu.observe(&[process(1, 0, 1.0, 0.0), reused]);
        assert_eq!(cpu.seconds(), 10.5);
    }

    #[test]
    fn a_meter_reports_peak_memory_cores_and_growth() {
        let meter = Meter::new(Need::DEFAULT);
        assert_eq!(meter.report(), json!({}), "nothing measured, nothing said");
        let start = Instant::now();
        meter.observe(&[process(1, 0, 0.0, 0.0), process(2, 1, 0.0, 0.0)], start);
        meter.observe(
            &[process(1, 0, 3.0, 0.0)],
            start + std::time::Duration::from_secs(2),
        );
        assert_eq!(meter.rss(), 100);
        meter.watch_disk(Path::new("/nonexistent"), 1_000);
        meter.disk_after(400);
        let body = meter.attach(json!({ "status": "succeeded" }));
        assert_eq!(body["status"], "succeeded");
        assert_eq!(body["peakMemoryBytes"], 200);
        assert_eq!(body["cpuCores"], 1.5);
        assert_eq!(body["diskGrowthBytes"], 0);
    }

    #[test]
    fn a_meter_walks_the_growth_of_its_checkout() {
        let dir = std::env::temp_dir().join(format!("hulahoop-grown-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("before"), [0u8; 40]).unwrap();
        let meter = Meter::new(Need::DEFAULT);
        meter.sample_disk();
        assert_eq!(meter.live_use().grown, 0, "nothing watched, nothing grown");
        meter.watch_disk(&dir, dir_size(&dir));
        std::fs::write(dir.join("built"), [0u8; 100]).unwrap();
        meter.sample_disk();
        assert_eq!(meter.live_use().grown, 100);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn capacity_keeps_the_reserve_and_room_for_live_runs() {
        let host = Host {
            memory_total: 16 * GIB,
            memory_available: 10 * GIB,
            disk_total: 100 * GIB,
            disk_available: 30 * GIB,
            cores: 8.0,
        };
        let live = [LiveUse {
            need: Need {
                memory_bytes: 3 * GIB,
                disk_bytes: 4 * GIB,
                cpu_cores: 1.5,
            },
            rss: GIB,
            grown: GIB,
        }];
        let room = capacity(&host, &live, 4);
        let reserve = 16 * GIB / 100 * 15;
        assert_eq!(room["memoryBytes"], 10 * GIB - reserve - 2 * GIB);
        // The gigabyte the checkout grew is already used on the disk, so
        // only the rest of the need is kept for it.
        assert_eq!(room["diskBytes"], 30 * GIB - 10 * GIB - 3 * GIB);
        assert_eq!(room["cpuCores"], 6.5);
        assert_eq!(room["maxRuns"], 4);
        assert_eq!(room["running"], 1);
        assert_eq!(room["pressure"], false);
        // With the run gone, its memory and its checkout's growth come back.
        assert_eq!(room["idleMemoryBytes"], 11 * GIB - reserve);
        assert_eq!(room["idleDiskBytes"], 31 * GIB - 10 * GIB);

        // Inside the reserve the host is under pressure, and says so: a live
        // run that merely reserved the room is not the same thing.
        let pressed = Host {
            memory_available: GIB,
            ..host
        };
        assert_eq!(capacity(&pressed, &[], 4)["memoryBytes"], 0);
        assert_eq!(capacity(&pressed, &[], 4)["pressure"], true);
        let reserved = [LiveUse {
            need: Need {
                memory_bytes: 64 * GIB,
                ..live[0].need
            },
            ..live[0]
        }];
        let full = capacity(&host, &reserved, 4);
        assert_eq!(full["memoryBytes"], 0);
        assert_eq!(full["pressure"], false);

        // A big disk keeps 10 GiB, a small one 5 GiB.
        let big = Host {
            disk_total: 200 * GIB,
            ..host
        };
        assert_eq!(disk_reserve(&big), 10 * GIB);
        let small = Host {
            disk_total: 20 * GIB,
            ..host
        };
        assert_eq!(disk_reserve(&small), 5 * GIB);
    }

    #[test]
    fn a_claim_envelope_without_a_need_reserves_the_defaults() {
        assert_eq!(Need::from_envelope(&Value::Null), Need::DEFAULT);
        let need =
            Need::from_envelope(&json!({ "memoryBytes": 5, "diskBytes": 6, "cpuCores": 0.5 }));
        assert_eq!(
            need,
            Need {
                memory_bytes: 5,
                disk_bytes: 6,
                cpu_cores: 0.5
            }
        );
    }
}
