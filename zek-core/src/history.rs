//! Versioned, isolated per-run history. Payloads deliberately omit arguments,
//! environment, prompts and process output; future secret providers must redact
//! any payloads they introduce before persistence.
use crate::execution::FlowReport;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub const SCHEMA_VERSION: u32 = 1;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn default_days() -> u64 {
    30
}
fn default_runs() -> usize {
    1000
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryConfig {
    #[serde(default)]
    pub directory: Option<PathBuf>,
    #[serde(default = "default_days")]
    pub retention_days: u64,
    #[serde(default = "default_runs")]
    pub max_runs: usize,
}
impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            directory: None,
            retention_days: default_days(),
            max_runs: default_runs(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub schema_version: u32,
    pub run_id: String,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub flow: String,
    pub kind: String,
    pub step: Option<String>,
    pub attempt: Option<u32>,
    pub status: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    pub duration_ms: Option<u64>,
    pub exit_code: Option<i32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub schema_version: u32,
    pub run_id: String,
    pub flow: String,
    pub status: String,
    pub started_at_ms: u64,
    pub finished_at_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub failed_steps: Vec<String>,
    #[serde(default)]
    pub skipped_steps: Vec<String>,
    #[serde(default)]
    pub excluded_steps: Vec<String>,
    #[serde(default)]
    pub incomplete: bool,
}
#[derive(Debug, Clone)]
pub struct HistoryStore {
    pub directory: PathBuf,
    pub retention_days: u64,
    pub max_runs: usize,
}
impl HistoryStore {
    pub fn new(directory: PathBuf, retention_days: u64, max_runs: usize) -> Self {
        Self {
            directory,
            retention_days,
            max_runs,
        }
    }
    pub fn start(&self, flow: &str) -> io::Result<HistoryRun> {
        fs::create_dir_all(&self.directory)?;
        self.prune()?;
        let (run_id, directory) = loop {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let id = format!(
                "{nanos:032x}-{:x}-{:x}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            );
            let directory = self.directory.join(&id);
            match fs::create_dir(&directory) {
                Ok(()) => break (id, directory),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        };
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(directory.join("events.jsonl"))?;
        let summary = RunSummary {
            schema_version: SCHEMA_VERSION,
            run_id,
            flow: flow.into(),
            status: "running".into(),
            started_at_ms: now_ms(),
            finished_at_ms: None,
            duration_ms: None,
            failed_steps: vec![],
            skipped_steps: vec![],
            excluded_steps: vec![],
            incomplete: false,
        };
        let run = HistoryRun(Arc::new(Mutex::new(RunWriter {
            file,
            directory,
            summary,
            sequence: 0,
            finished: false,
        })));
        {
            let writer = run.0.lock().unwrap();
            writer.save_summary()?;
        }
        run.event(flow, None, None, "run_started", Some("running"), None, None)?;
        Ok(run)
    }
    fn run_path(&self, id: &str) -> io::Result<PathBuf> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                crate::lang::messages().history_invalid_id,
            ));
        }
        let path = self.directory.join(id);
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                crate::lang::messages().history_symlink,
            ));
        }
        Ok(path)
    }
    pub fn events(&self, id: &str) -> io::Result<Vec<Event>> {
        let text = fs::read_to_string(self.run_path(id)?.join("events.jsonl"))?;
        // A crash may leave the final JSONL record incomplete. Earlier records
        // remain readable; malformed records are omitted without rewriting data.
        Ok(text
            .lines()
            .filter_map(|line| serde_json::from_str::<Event>(line).ok())
            .filter(|e| e.run_id == id)
            .collect())
    }
    pub fn summary(&self, id: &str) -> io::Result<RunSummary> {
        let path = self.run_path(id)?;
        if let Ok(bytes) = fs::read(path.join("summary.json")) {
            if let Ok(summary) = serde_json::from_slice::<RunSummary>(&bytes) {
                if summary.run_id == id {
                    return Ok(summary);
                }
            }
        }
        let events = self.events(id)?;
        let first = events.first().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                crate::lang::messages().history_no_records,
            )
        })?;
        let last = events.iter().rev().find(|e| e.kind == "run_finished");
        Ok(RunSummary {
            schema_version: SCHEMA_VERSION,
            run_id: id.into(),
            flow: first.flow.clone(),
            status: last
                .and_then(|e| e.status.clone())
                .unwrap_or_else(|| "incomplete".into()),
            started_at_ms: first.timestamp_ms,
            finished_at_ms: last.map(|e| e.timestamp_ms),
            duration_ms: last.and_then(|e| e.duration_ms),
            failed_steps: vec![],
            skipped_steps: vec![],
            excluded_steps: vec![],
            incomplete: true,
        })
    }
    pub fn list(
        &self,
        flow: Option<&str>,
        status: Option<&str>,
        limit: usize,
    ) -> io::Result<Vec<RunSummary>> {
        let entries = match fs::read_dir(&self.directory) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        let mut summaries = vec![];
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            if let Some(id) = entry.file_name().to_str() {
                if let Ok(summary) = self.summary(id) {
                    if flow.map_or(true, |f| f == summary.flow)
                        && status.map_or(true, |s| s == summary.status)
                    {
                        summaries.push(summary);
                    }
                }
            }
        }
        summaries.sort_by(|a, b| {
            b.started_at_ms
                .cmp(&a.started_at_ms)
                .then_with(|| b.run_id.cmp(&a.run_id))
        });
        summaries.truncate(limit);
        Ok(summaries)
    }
    /// Prune only completed records. Active/incomplete runs are never removed.
    /// Zero disables the corresponding age/count limit.
    pub fn prune(&self) -> io::Result<()> {
        let summaries = self.list(None, None, usize::MAX)?;
        let now = now_ms();
        let mut completed = 0;
        for summary in summaries {
            if summary.incomplete {
                continue;
            }
            let Some(end) = summary.finished_at_ms else {
                continue;
            };
            completed += 1;
            let old = self.retention_days > 0
                && now.saturating_sub(end) > self.retention_days.saturating_mul(86_400_000);
            let excess = self.max_runs > 0 && completed > self.max_runs;
            if old || excess {
                match fs::remove_dir_all(self.directory.join(&summary.run_id)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct HistoryRun(Arc<Mutex<RunWriter>>);
impl HistoryRun {
    pub fn id(&self) -> String {
        self.0.lock().unwrap().summary.run_id.clone()
    }
    #[allow(clippy::too_many_arguments)]
    pub fn event(
        &self,
        flow: &str,
        step: Option<&str>,
        attempt: Option<u32>,
        kind: &str,
        status: Option<&str>,
        duration_ms: Option<u64>,
        exit_code: Option<i32>,
    ) -> io::Result<()> {
        let mut w = self.0.lock().unwrap();
        let event = Event {
            schema_version: SCHEMA_VERSION,
            run_id: w.summary.run_id.clone(),
            sequence: w.sequence,
            timestamp_ms: now_ms(),
            flow: flow.into(),
            step: step.map(String::from),
            attempt,
            kind: kind.into(),
            status: status.map(String::from),
            reason: None,
            duration_ms,
            exit_code,
        };
        w.write_event(&event)
    }
    pub fn skip(&self, flow: &str, step: &str, reason: &str) -> io::Result<()> {
        let mut writer = self.0.lock().unwrap();
        let event = Event {
            schema_version: SCHEMA_VERSION,
            run_id: writer.summary.run_id.clone(),
            sequence: writer.sequence,
            timestamp_ms: now_ms(),
            flow: flow.into(),
            kind: "step_skipped".into(),
            step: Some(step.into()),
            attempt: None,
            status: Some("skipped".into()),
            reason: Some(reason.into()),
            duration_ms: None,
            exit_code: None,
        };
        writer.write_event(&event)
    }

    pub fn finish(&self, status: &str, report: Option<&FlowReport>) -> io::Result<()> {
        self.0.lock().unwrap().finish(status, report)
    }
}
#[derive(Debug)]
struct RunWriter {
    file: File,
    directory: PathBuf,
    summary: RunSummary,
    sequence: u64,
    finished: bool,
}
impl RunWriter {
    fn write_event(&mut self, event: &Event) -> io::Result<()> {
        if self.finished {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                crate::lang::messages().history_run_finished,
            ));
        }
        serde_json::to_writer(&mut self.file, event)?;
        self.file.write_all(b"\n")?;
        self.file.flush()?;
        self.sequence += 1;
        Ok(())
    }
    fn save_summary(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&self.summary)?;
        fs::write(self.directory.join("summary.tmp"), bytes)?;
        fs::rename(
            self.directory.join("summary.tmp"),
            self.directory.join("summary.json"),
        )
    }
    fn finish(&mut self, status: &str, report: Option<&FlowReport>) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        let end = now_ms();
        let duration = end.saturating_sub(self.summary.started_at_ms);
        self.summary.status = status.into();
        self.summary.finished_at_ms = Some(end);
        self.summary.duration_ms = Some(duration);
        if let Some(report) = report {
            self.summary.failed_steps = report.failed_steps.clone();
            self.summary.skipped_steps = report.skipped_steps.clone();
            self.summary.excluded_steps = report.excluded_steps.clone();
        }
        let event = Event {
            schema_version: SCHEMA_VERSION,
            run_id: self.summary.run_id.clone(),
            sequence: self.sequence,
            timestamp_ms: end,
            flow: self.summary.flow.clone(),
            kind: "run_finished".into(),
            step: None,
            attempt: None,
            status: Some(status.into()),
            reason: report.map(|report| report.exit_reason.clone()),
            duration_ms: Some(duration),
            exit_code: report.map(FlowReport::exit_code),
        };
        self.write_event(&event)?;
        self.finished = true;
        self.save_summary()
    }
}
impl Drop for RunWriter {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.finish("cancelled", None);
        }
    }
}
