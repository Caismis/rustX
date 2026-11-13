//! One Template evaluation in the existing owned process-supervision domain.
//!
//! `MiniJinja` fuel counts VM instructions, not native filters or allocations.
//! Only this worker compiles/evaluates source. The server performs parser-only
//! syntax admission, retains bounded input/output, and awaits physical reaping.
use super::{BTreeMap, MAX_VALUE_BYTES, Value, WorkflowRunError, WorkflowValue, runtime};
use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::process_runner::{
    ProcessOutcomeIntent, RunnerBackedProcessRunner, RunnerTestControl, SupervisedCommandSpec,
    SupervisedProcessRunner,
};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

const WALL_LIMIT: Duration = Duration::from_secs(2);
// A per-worker limit must also bound the server's aggregate renderer memory.
// The permit belongs to the physical driver through cleanup, not its waiter.
static WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub(in crate::runtime::workflow) async fn render_isolated(
    template: &str,
    variables: &BTreeMap<String, WorkflowValue>,
    mut resolve: impl FnMut(&WorkflowValue) -> Result<Value, WorkflowRunError>,
    cancellation: &ExecutionCancellation,
) -> Result<Value, WorkflowRunError> {
    let context = resolve(&WorkflowValue::Object {
        fields: variables.clone(),
    })?;
    render_with_control(template, &context, cancellation, None).await
}

pub(super) async fn render_with_control(
    template: &str,
    context: &Value,
    cancellation: &ExecutionCancellation,
    control: Option<RunnerTestControl>,
) -> Result<Value, WorkflowRunError> {
    if cancellation.is_cancelled() {
        return Err(WorkflowRunError::from_cancellation(cancellation));
    }
    if template.len() > MAX_VALUE_BYTES {
        return Err(runtime("template exceeds 64 KiB"));
    }
    crate::runtime::workflow::expressions::bounded_value_bytes(context)?;
    let permit = tokio::select! {
        biased;
        () = cancellation.cancelled() => return Err(WorkflowRunError::from_cancellation(cancellation)),
        permit = WORKERS.acquire() => permit.map_err(|error| runtime(error.to_string()))?,
    };
    let executable = renderer_binary();
    let executable = executable
        .to_str()
        .ok_or_else(|| runtime("invalid renderer executable path"))?;
    if executable.len() > 8192 {
        return Err(runtime("renderer executable path exceeds its bound"));
    }
    let command = format!("exec '{}'", executable.replace('\'', "'\\''"));
    let context = serde_json::to_vec(context).map_err(|error| runtime(error.to_string()))?;
    let source = template.as_bytes().to_vec();
    let input = tokio::task::spawn_blocking(move || {
        let input = tempfile::tempdir()?;
        std::fs::write(input.path().join("source"), source)?;
        std::fs::write(input.path().join("context"), context)?;
        Ok::<_, std::io::Error>(input)
    })
    .await
    .map_err(|error| runtime(error.to_string()))?
    .map_err(|error| runtime(error.to_string()))?;
    let spec = SupervisedCommandSpec {
        // Only the runtime-owned absolute executable path enters shell code.
        // The two fixed input files are private, bounded and never authored
        // filesystem selectors; using files avoids exec's per-argument limit.
        command,
        cwd: input.path().to_owned(),
        command_environment: Vec::new(),
        timeout: Some(WALL_LIMIT),
        cancellation: cancellation.child_signal(),
    };
    // The driver retains the input files through physical settlement even if
    // its result waiter is dropped. The existing runner owns every process;
    // this task only retains/cleans its private transport resources.
    let (send, receive) = tokio::sync::oneshot::channel();
    std::mem::drop(tokio::spawn(async move {
        let _permit = permit;
        let result = RunnerBackedProcessRunner
            .run(spec, control)
            .await;
        let cleaned = tokio::task::spawn_blocking(move || input.close()).await;
        let result = match cleaned {
            Ok(Ok(())) => result,
            Ok(Err(error)) => Err(format!("cannot remove Template input: {error}")),
            Err(error) => Err(format!("Template input cleanup failed: {error}")),
        };
        let _ = send.send(result);
    }));
    let result = receive
        .await
        .map_err(|error| runtime(error.to_string()))?
        .map_err(|error| runtime(format!("Template renderer supervision failed: {error}")))?;
    // The shared runner has already settled and reaped the owned invocation.
    // Its control failure outranks a cancellation that became visible later.
    if let ProcessOutcomeIntent::ProcessControlFailed(message) = &result.intent {
        let detail = crate::runtime::workflow::bound_workflow_diagnostic(message);
        return Err(runtime(format!(
            "Template renderer supervision failed: {detail}"
        )));
    }
    if cancellation.is_cancelled() || result.intent == ProcessOutcomeIntent::Cancelled {
        return Err(WorkflowRunError::from_cancellation(cancellation));
    }
    if result.intent != ProcessOutcomeIntent::Completed || result.exit_code != Some(0) {
        let detail = crate::runtime::workflow::bound_workflow_diagnostic(String::from_utf8_lossy(
            &result.stderr,
        ));
        return Err(runtime(format!(
            "Template renderer failed or exceeded its resource limits: {detail}"
        )));
    }
    if result.stdout.len() > MAX_VALUE_BYTES {
        return Err(runtime("template output exceeds 64 KiB"));
    }
    let text = String::from_utf8(result.stdout).map_err(|error| runtime(error.to_string()))?;
    let value = serde_json::json!({"output":text});
    crate::runtime::workflow::expressions::bounded_value_bytes(&value)?;
    Ok(value)
}

fn renderer_binary() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_workflow-renderer") {
        return PathBuf::from(path);
    }
    let executable = std::env::current_exe().expect("current executable");
    let directory = executable.parent().expect("current executable directory");
    if directory.ends_with("deps") {
        directory
            .parent()
            .expect("binary directory")
            .join("workflow-renderer")
    } else {
        directory.join("workflow-renderer")
    }
}

/// Called only by the dedicated single-threaded renderer binary.
pub(in crate::runtime::workflow) fn run_worker() -> i32 {
    match worker() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!(
                "{}",
                crate::runtime::workflow::bound_workflow_diagnostic(error)
            );
            1
        }
    }
}

fn worker() -> Result<(), String> {
    use nix::sys::resource::{Resource, setrlimit};
    setrlimit(Resource::RLIMIT_CORE, 0, 0).map_err(|error| error.to_string())?;
    setrlimit(Resource::RLIMIT_CPU, 1, 1).map_err(|error| error.to_string())?;
    // The binary's capped allocator also enforces a portable 128 MiB live
    // requested-heap bound. Linux additionally limits all virtual mappings.
    #[cfg(target_os = "linux")]
    setrlimit(Resource::RLIMIT_AS, 512 * 1024 * 1024, 512 * 1024 * 1024)
        .map_err(|error| error.to_string())?;
    let source = read_input("source")?;
    let template = std::str::from_utf8(&source).map_err(|error| error.to_string())?;
    let context: Value =
        serde_json::from_slice(&read_input("context")?).map_err(|error| error.to_string())?;
    crate::runtime::workflow::expressions::bounded_value_bytes(&context)
        .map_err(|error| error.to_string())?;
    let result = super::render_context(template, &context).map_err(|error| error.to_string())?;
    let text = result["output"]
        .as_str()
        .ok_or("Template renderer produced no text")?;
    std::io::stdout()
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

fn read_input(name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(name)
        .map_err(|error| error.to_string())?
        .take((MAX_VALUE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_VALUE_BYTES {
        return Err("Template renderer input exceeds 64 KiB".into());
    }
    Ok(bytes)
}
