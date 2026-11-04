//! Code nodes lower to the ordinary admitted workspace Bash invocation.
use super::{
    BTreeMap, SchemaMap, Value, WorkflowCompileError, WorkflowRunError, WorkflowValue,
    WorkflowVarType,
};

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
            "Code must contain a Bash script without NUL bytes".into(),
        )
        .at("code"));
    }
    for (name, value) in variables {
        if !valid_name(name)
            || matches!(
                name.as_str(),
                "HOME"
                    | "home"
                    | "CODEX_HOME"
                    | "PATH"
                    | "IFS"
                    | "BASH_ENV"
                    | "ENV"
                    | "SHELLOPTS"
                    | "BASHOPTS"
            )
        {
            return Err(WorkflowCompileError::InvalidField(
                "Code input must be a non-reserved Bash variable name".into(),
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
    variables: &BTreeMap<String, WorkflowValue>,
    code: &str,
    mut evaluate: impl FnMut(&WorkflowValue) -> Result<Value, WorkflowRunError>,
) -> Result<Value, WorkflowRunError> {
    let mut command = String::new();
    // Literal assignments, never shell interpolation of upstream data. JSON
    // preserves the types of non-string inputs; strings retain their bytes.
    for (name, value) in variables {
        let value = evaluate(value)?;
        let text = match value {
            Value::String(text) => text,
            value => value.to_string(),
        };
        if text.contains('\0') {
            return Err(WorkflowRunError::InvalidValue(
                "Bash inputs cannot contain NUL bytes".into(),
            ));
        }
        command.push_str(name);
        command.push_str("='");
        command.push_str(&text.replace('\'', "'\\''"));
        command.push_str("'\n");
    }
    command.push_str(code);
    if command.len() > super::MAX_VALUE_BYTES {
        return Err(WorkflowRunError::InvalidValue(
            "Code command exceeds Workflow value bound".into(),
        ));
    }
    Ok(serde_json::json!({"command":command}))
}
