mod common;

#[cfg(unix)]
mod unix {
    use std::collections::HashMap;
    use std::path::Path;

    use super::common::write_fake_opencode;
    use zek_core::error::FlowFinalStatus;
    use zek_core::execution::FlowRunner;
    use zek_core::flows::Flow;

    #[tokio::test]
    async fn flujo_opencode_con_fake_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(
            tmp.path(),
            "echo '{\"sessionID\": \"sess-1\", \"type\": \"message\"}'",
        );

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: build\n    type: command\n    command: echo hola\n  - name: review\n    type: opencode\n    prompt: 'Resultado: {{steps.build.stdout}}'\n",
            Path::new("test.yaml"),
        )
        .unwrap();

        let commands = HashMap::new();
        let runner = FlowRunner::with_opencode(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        assert_eq!(report.status, FlowFinalStatus::Success);
        assert!(report.results.get("review").unwrap().status.is_success());
    }

    #[tokio::test]
    async fn opencode_que_falla_tumba_el_flujo() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(tmp.path(), "echo 'error' >&2\nexit 1");

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: x\n    type: opencode\n    prompt: hola\n",
            Path::new("test.yaml"),
        )
        .unwrap();

        let commands = HashMap::new();
        let runner = FlowRunner::with_opencode(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        assert_eq!(report.status, FlowFinalStatus::Failed);
    }

    #[tokio::test]
    async fn sesion_se_continua_entre_steps_opencode() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = write_fake_opencode(
            tmp.path(),
            "echo \"ARGS: $@\" >&2\necho '{\"sessionID\": \"sess-9\"}'",
        );

        let flow = Flow::from_str(
            "name: f\nsteps:\n  - name: primero\n    type: opencode\n    prompt: hola\n  - name: segundo\n    type: opencode\n    prompt: continuo\n    continue_session: true\n",
            Path::new("test.yaml"),
        )
        .unwrap();

        let commands = HashMap::new();
        let runner = FlowRunner::with_opencode(
            &flow,
            &commands,
            tmp.path().to_path_buf(),
            false,
            fake.to_str().unwrap(),
        );
        let report = runner.run().await.unwrap();

        let segundo = &report.results.get("segundo").unwrap().status;
        assert!(segundo.stderr().contains("--session sess-9"));
    }
}
