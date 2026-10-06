use std::{
    fs,
    time::{Duration, Instant},
};
use zek_core::watch::{glob_matches, PollWatcher, WatchOptions};
#[test]
fn glob_matching_supports_paths_zero_directory_double_star_and_unicode() {
    for (pattern, path, expected) in [
        ("**/*.rs", "main.rs", true),
        ("**/*.rs", "src/nested/main.rs", true),
        ("src/*.rs", "src/nested/main.rs", false),
        ("src/?ain.rs", "src/main.rs", true),
        ("**", "a/b", true),
        ("generated/**", "generated/", true),
        ("src/?", "src/ñ", true),
    ] {
        assert_eq!(glob_matches(pattern, path), expected, "{pattern} {path}");
    }
}
#[test]
fn grouped_changes_are_debounced_once_and_changes_during_work_queue_once() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("input.txt");
    fs::write(&file, "initial").unwrap();
    let mut watcher = PollWatcher::new(
        tmp.path().into(),
        WatchOptions {
            debounce: Duration::from_millis(100),
            ..Default::default()
        },
    )
    .unwrap();
    let now = Instant::now();
    fs::write(&file, "one").unwrap();
    assert!(watcher.poll(now).unwrap());
    fs::write(&file, "two").unwrap();
    assert!(watcher.poll(now + Duration::from_millis(50)).unwrap());
    assert!(!watcher.take_ready(now + Duration::from_millis(100)));
    assert!(watcher.take_ready(now + Duration::from_millis(151)));
    assert!(!watcher.take_ready(now + Duration::from_secs(1)));
    // Polling while a caller's run is active never consumes the pending rerun.
    for i in 0..4 {
        fs::write(&file, i.to_string()).unwrap();
        watcher.poll(now + Duration::from_secs(2)).unwrap();
    }
    assert!(watcher.take_ready(now + Duration::from_secs(3)));
    assert!(!watcher.take_ready(now + Duration::from_secs(3)));
}
#[test]
fn default_and_explicit_exclusions_prevent_feedback_and_includes_filter_files() {
    let tmp = tempfile::tempdir().unwrap();
    let history = tmp.path().join("custom-history");
    let log = tmp.path().join("events.jsonl");
    let options = WatchOptions {
        include: vec!["**/*.txt".into(), "**/*.jsonl".into()],
        exclude: vec!["generated/**".into()],
        ignored_paths: vec![history.clone(), log.clone()],
        debounce: Duration::ZERO,
    };
    let mut watcher = PollWatcher::new(tmp.path().into(), options).unwrap();
    for directory in [
        "target",
        ".git",
        ".zek",
        "history",
        "logs",
        "cache",
        ".cache",
        "generated",
        "custom-history",
    ] {
        fs::create_dir_all(tmp.path().join(directory)).unwrap();
        fs::write(tmp.path().join(directory).join("file.txt"), "changed").unwrap();
    }
    fs::write(&log, "changed").unwrap();
    fs::write(tmp.path().join("output.log"), "changed").unwrap();
    fs::write(tmp.path().join("ignored.rs"), "changed").unwrap();
    assert!(!watcher.poll(Instant::now()).unwrap());
    fs::write(tmp.path().join("included.txt"), "changed").unwrap();
    assert!(watcher.poll(Instant::now()).unwrap());
    assert!(watcher.take_ready(Instant::now()));
    fs::remove_file(tmp.path().join("included.txt")).unwrap();
    assert!(watcher.poll(Instant::now()).unwrap());
}
#[cfg(unix)]
#[test]
fn symlink_directories_are_not_followed() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), tmp.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(tmp.path(), tmp.path().join("loop")).unwrap();
    let mut watcher = PollWatcher::new(tmp.path().into(), WatchOptions::default()).unwrap();
    fs::write(outside.path().join("external"), "changed").unwrap();
    assert!(!watcher.poll(Instant::now()).unwrap());
}

#[test]
fn newly_configured_history_directory_can_be_excluded_without_a_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let mut watcher = PollWatcher::new(tmp.path().into(), WatchOptions::default()).unwrap();
    let directory = tmp.path().join("new-records");
    watcher.ignore_path(directory.clone()).unwrap();
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("events.jsonl"), "history").unwrap();
    assert!(!watcher.poll(Instant::now()).unwrap());
    fs::write(tmp.path().join("input.txt"), "input").unwrap();
    assert!(watcher.poll(Instant::now()).unwrap());
}
