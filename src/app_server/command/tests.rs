use super::*;

fn parameters() -> ExecParams {
    ExecParams {
        session_id: crate::local_runtime::session::SessionId::generate(),
        command: vec!["/bin/echo".into(), "hi".into()],
        cwd: "/workspace".into(),
        env: BTreeMap::new(),
        process_id: Some("fragmented".into()),
        tty: false,
        stream_stdin: false,
        stream_stdout_stderr: false,
        output_bytes_cap: None,
        timeout_ms: None,
        size: None,
    }
}

#[test]
fn validate_accepts_the_bounded_shape() {
    assert!(validate(&parameters()).is_ok());
}

#[test]
fn validate_rejects_malformed_parameters() {
    let mut params = parameters();
    params.command = Vec::new();
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.command[0] = String::new();
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.command[1] = "a\0b".into();
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.timeout_ms = Some(0);
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.timeout_ms = Some(3_600_001);
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.output_bytes_cap = Some(0);
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.output_bytes_cap = Some(262_145);
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.size = Some(TerminalSize { rows: 0, cols: 80 });
    params.tty = true;
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.size = Some(TerminalSize { rows: 24, cols: 80 });
    assert!(validate(&params).is_err());
    // Streaming or tty requires an explicit process identity.
    let mut params = parameters();
    params.tty = true;
    params.process_id = None;
    assert!(validate(&params).is_err());
    let mut params = parameters();
    params.stream_stdin = true;
    params.process_id = None;
    assert!(validate(&params).is_err());
}

#[test]
fn validate_rejects_runtime_owned_environment_keys() {
    for key in [
        "PATH",
        "HOME",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_KEY_9",
        "GIT_PAGER",
    ] {
        let mut params = parameters();
        params.env.insert(key.into(), Some("x".into()));
        assert!(
            validate(&params).is_err(),
            "runtime-owned key must be refused: {key}"
        );
    }
    let mut params = parameters();
    params.env.insert("AUTHORIZED".into(), Some("value".into()));
    assert!(validate(&params).is_ok());
}

#[tokio::test]
async fn controls_on_an_unknown_or_closed_process_are_refused() {
    let commands = Arc::new(Commands::default());
    assert_eq!(
        commands.terminate("missing"),
        Err(domain(ErrorData::EnvironmentResourceNotFound))
    );
    assert!(
        commands
            .resize("missing", TerminalSize { rows: 1, cols: 1 })
            .await
            .is_err()
    );
    commands.close();
    assert_eq!(
        commands.terminate("missing"),
        Err(domain(ErrorData::ConnectionClosed))
    );
}
