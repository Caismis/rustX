//! Typed Python/JavaScript main functions over the admitted workspace Bash tool.
use super::{
    BTreeMap, SchemaMap, Value, WorkflowCompileError, WorkflowRunError, WorkflowValue,
    WorkflowVarType,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, super::Serialize, super::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowCodeLanguage {
    Python3,
    JavaScript,
}

const PYTHON_RUNNER: &str = include_str!("code/python_runner.py");
const JAVASCRIPT_RUNNER: &str = include_str!("code/javascript_runner.cjs");

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().enumerate().all(|(index, ch)| {
            ch.is_ascii_alphabetic() || ch == b'_' || index > 0 && ch.is_ascii_digit()
        })
}

pub(super) fn output_schema(
    outputs: &BTreeMap<String, WorkflowVarType>,
) -> Result<Value, WorkflowCompileError> {
    if outputs.is_empty() || outputs.keys().any(|name| !valid_name(name)) {
        return Err(
            WorkflowCompileError::InvalidField("Code outputs require named fields".into())
                .at("outputs"),
        );
    }
    let properties = outputs
        .iter()
        .map(|(name, kind)| (name.clone(), kind.schema()))
        .collect::<serde_json::Map<_, _>>();
    Ok(
        serde_json::json!({"type":"object","properties":properties,"required":outputs.keys().collect::<Vec<_>>(),"additionalProperties":false}),
    )
}

pub(super) fn validate(
    variables: &BTreeMap<String, WorkflowValue>,
    code: &str,
    scope: &SchemaMap,
    id: &str,
) -> Result<(), WorkflowCompileError> {
    if code.trim().is_empty() || code.contains('\0') {
        return Err(WorkflowCompileError::InvalidField(
            "Code must contain source without NUL bytes".into(),
        )
        .at("code"));
    }
    for (name, value) in variables {
        if !valid_name(name) {
            return Err(WorkflowCompileError::InvalidField(
                "Code input must be an identifier".into(),
            )
            .at(format!("variables.{name}")));
        }
        if !matches!(value, WorkflowValue::Reference { .. }) {
            return Err(WorkflowCompileError::InvalidField(
                "Code input must select a variable".into(),
            )
            .at(format!("variables.{name}")));
        }
        super::value_schema(value, scope, id, 0)
            .map_err(|error| error.at(format!("variables.{name}")))?;
    }
    Ok(())
}

pub(super) fn arguments(
    language: WorkflowCodeLanguage,
    variables: &BTreeMap<String, WorkflowValue>,
    code: &str,
    mut evaluate: impl FnMut(&WorkflowValue) -> Result<Value, WorkflowRunError>,
) -> Result<Value, WorkflowRunError> {
    let inputs = variables
        .iter()
        .map(|(name, value)| evaluate(value).map(|value| (name.clone(), value)))
        .collect::<Result<serde_json::Map<_, _>, _>>()?;
    // A compact JSON line in a quoted heredoc keeps source and inputs as data
    // until their language interpreter receives them; no argv-size limitation
    // or shell interpolation is introduced. The line always starts with `{`,
    // so it cannot terminate this fixed, standalone heredoc delimiter.
    let payload = serde_json::json!({"code":code,"inputs":inputs}).to_string();
    let (interpreter, runner) = match language {
        WorkflowCodeLanguage::Python3 => ("python3 -X utf8 -c", PYTHON_RUNNER),
        WorkflowCodeLanguage::JavaScript => ("node --input-type=commonjs -e", JAVASCRIPT_RUNNER),
    };
    let command = format!(
        "{interpreter} '{}' <<'RUSTX_CODE_PAYLOAD'\n{payload}\nRUSTX_CODE_PAYLOAD\n",
        runner.replace('\'', "'\\''")
    );
    if command.len() > super::MAX_VALUE_BYTES {
        return Err(WorkflowRunError::InvalidValue(
            "Code command exceeds Workflow value bound".into(),
        ));
    }
    Ok(serde_json::json!({"command":command}))
}
