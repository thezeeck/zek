#![cfg(windows)]

use std::collections::HashMap;
use std::time::Duration;
use zek_core::exec::{run_program, CommandExecutor};

#[tokio::test]
async fn suspended_child_is_resumed_after_job_assignment() {
    let status = CommandExecutor::new("echo ready")
        .timeout(Duration::from_secs(5))
        .execute()
        .await;
    assert!(status.is_success(), "{status:?}");
    assert_eq!(status.stdout().trim(), "ready");
}

#[tokio::test]
async fn cancellation_terminates_windows_job_descendants() {
    let tmp = tempfile::tempdir().unwrap();
    let args = vec![
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
        // El marcador solo aparece cuando el nieto arrancó dentro del job.
        r#"$child = Start-Process -FilePath "$PSHOME\powershell.exe" -WorkingDirectory $pwd -NoNewWindow -PassThru -ArgumentList '-NoProfile', '-NonInteractive', '-Command', 'Set-Content ready yes; Start-Sleep -Seconds 2; Set-Content survived yes'; $child.WaitForExit()"#.into(),
    ];
    let env = HashMap::new();
    let mut execution = Box::pin(run_program(
        "powershell.exe",
        &args,
        Some(tmp.path()),
        &env,
        Duration::from_secs(15),
        false,
    ));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            tokio::select! {
                status = &mut execution => panic!("process exited before cancellation: {status:?}"),
                _ = tokio::time::sleep(Duration::from_millis(20)) => {
                    if tmp.path().join("ready").exists() {
                        break;
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    drop(execution);
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(!tmp.path().join("survived").exists());
}
