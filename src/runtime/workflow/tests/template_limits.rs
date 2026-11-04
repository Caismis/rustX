use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
use crate::runtime::process_runner::RunnerTestControl;
use crate::runtime::types::CancellationReason;
use crate::runtime::workflow::transform;
use crate::runtime::workflow::{WorkflowRunError, WorkflowValue};
use serde_json::json;
use std::collections::BTreeMap;
use std::time::Duration;

fn cancellation() -> (CancellationSignal, ExecutionCancellation) {
    let signal = CancellationSignal::new();
    let view = ExecutionCancellation::detached(signal.clone(), CancellationReason::UserRequested);
    (signal, view)
}

#[test]
fn empty_substring_count_uses_python_character_boundaries() {
    for (source, expected) in [
        ("{{ ''.count('') }}", "1"),
        ("{{ 'x'.count('') }}", "2"),
        ("{{ '你好'.count('') }}", "3"),
        ("{{ 'banana'.count('an') }}", "2"),
    ] {
        let result = transform::render(source, &BTreeMap::new(), |_| Ok(json!({}))).unwrap();
        assert_eq!(result["output"], expected, "{source}");
    }
}

#[test]
fn template_admission_only_parses_potentially_expanding_constants() {
    let source = "{{ ('x' * 100000000) ~ ('x' * 100000000) }}";
    let (result, effects) = crate::local_runtime::static_effects::measure(|| {
        transform::validate_template(source, &BTreeMap::new())
    });
    result.unwrap();
    assert_eq!(effects, [0; 11], "syntax admission must stay inert");
    assert!(transform::validate_template("{% if %}", &BTreeMap::new()).is_err());
}

async fn isolated(source: &str) -> Result<serde_json::Value, WorkflowRunError> {
    let (_, cancellation) = cancellation();
    tokio::time::timeout(
        Duration::from_secs(12),
        transform::render_isolated(source, &BTreeMap::new(), |_| Ok(json!({})), &cancellation),
    )
    .await
    .expect("renderer must physically settle within a finite deadline")
}

#[tokio::test]
async fn native_filter_allocations_and_long_computations_cannot_escape_the_worker() {
    for source in [
        "{{ {} | tojson(indent=9223372036854775807) }}",
        "{{ 'x' | indent(width=9223372036854775807) }}",
        "{{ ([1] * 1000000000000) | join(',') }}",
        "{{ ([1] * 1000000000000) | sum }}",
        "{{ ('x' * 100000000) ~ ('x' * 100000000) }}",
        "{% set text %}{% for i in range(10000) %}{{ 'x' * 60000 }}{% endfor %}{% endset %}{{ text }}",
    ] {
        assert!(isolated(source).await.is_err(), "{source}");
        assert_eq!(
            isolated("healthy").await.unwrap(),
            json!({"output":"healthy"})
        );
    }
}

#[tokio::test]
async fn isolated_rendering_keeps_jinja_methods_unicode_and_full_sized_inputs() {
    assert_eq!(
        isolated(
            "{% set text = '你好' %}{{ text.upper() }}|{{ text.count('') }}|{{ [1,2,3] | sum }}"
        )
        .await
        .unwrap(),
        json!({"output":"你好|3|6"}),
    );
    let padding = " ".repeat(transform::MAX_VALUE_BYTES - 30);
    let source = format!("{{# {padding} #}}{{{{ text | length }}}}");
    let text = "x".repeat(transform::MAX_VALUE_BYTES - 32);
    let (_, cancellation) = cancellation();
    let result = transform::render_isolated(
        &source,
        &BTreeMap::from([("text".into(), WorkflowValue::Literal { value: json!(text) })]),
        |_| Ok(json!({"text":text})),
        &cancellation,
    )
    .await
    .unwrap();
    assert_eq!(result, json!({"output":text.len().to_string()}));
}

#[tokio::test]
async fn cancellation_waits_for_the_owned_renderer_to_be_reaped() {
    let control = RunnerTestControl::new();
    let lifecycle = control.lifecycle.clone();
    let (signal, cancellation) = cancellation();
    let running = tokio::spawn(async move {
        transform::template_worker::render_with_control(
            "{{ ([1] * 1000000000000) | sum }}",
            &json!({}),
            &cancellation,
            Some(control),
        )
        .await
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        lifecycle.await_ownership_established(),
    )
    .await
    .unwrap();
    signal.cancel();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(12), running)
            .await
            .unwrap()
            .unwrap(),
        Err(WorkflowRunError::Cancelled(
            CancellationReason::UserRequested
        )),
    ));
    tokio::time::timeout(
        Duration::from_secs(1),
        lifecycle.await_direct_child_reaped(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn dropping_the_result_waiter_does_not_drop_the_renderer_owner() {
    let control = RunnerTestControl::new();
    let lifecycle = control.lifecycle.clone();
    let (_, cancellation) = cancellation();
    let running = tokio::spawn(async move {
        transform::template_worker::render_with_control(
            "{{ ([1] * 1000000000000) | sum }}",
            &json!({}),
            &cancellation,
            Some(control),
        )
        .await
    });
    tokio::time::timeout(
        Duration::from_secs(5),
        lifecycle.await_ownership_established(),
    )
    .await
    .unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        Duration::from_secs(12),
        lifecycle.await_direct_child_reaped(),
    )
    .await
    .expect("the detached physical owner must settle and reap after waiter drop");
}
