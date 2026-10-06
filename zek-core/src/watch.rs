//! Portable polling observer. One snapshot and debounce state per watched root.
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    fs,
    hash::Hasher,
    io::{self, Read},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub ignored_paths: Vec<PathBuf>,
    pub debounce: Duration,
}
impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            include: vec!["**".into()],
            exclude: vec![],
            ignored_paths: vec![],
            debounce: Duration::from_millis(300),
        }
    }
}
#[derive(Debug)]
pub struct PollWatcher {
    root: PathBuf,
    options: WatchOptions,
    snapshot: HashMap<PathBuf, u64>,
    last_change: Option<Instant>,
    pending: bool,
}
impl PollWatcher {
    pub fn new(root: PathBuf, options: WatchOptions) -> io::Result<Self> {
        let root = fs::canonicalize(root)?;
        let mut options = options;
        for path in &mut options.ignored_paths {
            if path.is_relative() {
                *path = std::env::current_dir()?.join(&*path);
            }
            *path = resolve_path(path)?;
        }
        let snapshot = scan(&root, &options)?;
        Ok(Self {
            root,
            options,
            snapshot,
            last_change: None,
            pending: false,
        })
    }
    /// Exclude a newly configured output location without generating a change.
    pub fn ignore_path(&mut self, path: PathBuf) -> io::Result<()> {
        let path = if path.is_relative() {
            std::env::current_dir()?.join(path)
        } else {
            path
        };
        let path = resolve_path(&path)?;
        self.snapshot.retain(|file, _| !file.starts_with(&path));
        if !self.options.ignored_paths.contains(&path) {
            self.options.ignored_paths.push(path);
        }
        Ok(())
    }

    /// Record changes without consuming a pending rerun, including during work.
    pub fn poll(&mut self, now: Instant) -> io::Result<bool> {
        let snapshot = scan(&self.root, &self.options)?;
        let changed = snapshot != self.snapshot;
        if changed {
            self.snapshot = snapshot;
            self.last_change = Some(now);
            self.pending = true;
        }
        Ok(changed)
    }
    /// Consume at most one rerun after the most recent observed change settles.
    pub fn take_ready(&mut self, now: Instant) -> bool {
        if self.pending
            && self
                .last_change
                .is_some_and(|last| now.saturating_duration_since(last) >= self.options.debounce)
        {
            self.pending = false;
            true
        } else {
            false
        }
    }
}
fn resolve_path(path: &Path) -> io::Result<PathBuf> {
    let normalized = normalize(path);
    let mut existing = normalized.as_path();
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(existing) {
            Ok(mut resolved) => {
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = existing.file_name().ok_or(error)?;
                suffix.push(name.to_os_string());
                existing = existing.parent().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        crate::lang::messages().watch_no_ancestor,
                    )
                })?;
            }
            Err(error) => return Err(error),
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}
fn scan(root: &Path, options: &WatchOptions) -> io::Result<HashMap<PathBuf, u64>> {
    let mut snapshot = HashMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            // Never follow symlinks: no escape from root, cycles or duplicate paths.
            if kind.is_symlink() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if excluded(&path, &relative, kind.is_dir(), options) {
                continue;
            }
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file()
                && options
                    .include
                    .iter()
                    .any(|pattern| glob_matches(pattern, &relative))
            {
                let mut file = match fs::File::open(&path) {
                    Ok(file) => file,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e),
                };
                let mut hash = DefaultHasher::new();
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = file.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    hash.write(&buffer[..count]);
                }
                snapshot.insert(path, hash.finish());
            }
        }
    }
    Ok(snapshot)
}
fn excluded(path: &Path, relative: &str, is_dir: bool, options: &WatchOptions) -> bool {
    if options
        .ignored_paths
        .iter()
        .any(|ignored| path.starts_with(ignored))
    {
        return true;
    }
    if relative.split('/').any(|part| {
        matches!(
            part,
            "target" | ".git" | ".zek" | "history" | "logs" | "cache" | ".cache"
        )
    }) || relative.ends_with(".log")
    {
        return true;
    }
    options.exclude.iter().any(|pattern| {
        glob_matches(pattern, relative)
            || (is_dir && glob_matches(pattern, &format!("{relative}/")))
    })
}
/// Supported glob syntax: `*` and `?` within components, `**` across components,
/// and `**/` matching zero or more directories. Paths use `/` on every platform.
pub fn glob_matches(pattern: &str, value: &str) -> bool {
    fn matches(
        p: &[char],
        v: &[char],
        i: usize,
        j: usize,
        memo: &mut HashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(&result) = memo.get(&(i, j)) {
            return result;
        }
        let result = if i == p.len() {
            j == v.len()
        } else if p[i] == '*' {
            let double = p.get(i + 1) == Some(&'*');
            let next = i + if double { 2 } else { 1 };
            matches(p, v, next, j, memo)
                || (double && p.get(next) == Some(&'/') && matches(p, v, next + 1, j, memo))
                || (j < v.len() && (double || v[j] != '/') && matches(p, v, i, j + 1, memo))
        } else {
            j < v.len()
                && ((p[i] == '?' && v[j] != '/') || p[i] == v[j])
                && matches(p, v, i + 1, j + 1, memo)
        };
        memo.insert((i, j), result);
        result
    }
    matches(
        &pattern.chars().collect::<Vec<_>>(),
        &value.chars().collect::<Vec<_>>(),
        0,
        0,
        &mut HashMap::new(),
    )
}
