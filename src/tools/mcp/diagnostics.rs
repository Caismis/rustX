//! Peer diagnostics are sanitized before native framing and byte bounds.
use crate::credentials::SourceCredentials;
use rmcp::model::ErrorData;
use rmcp::service::ServiceError;

pub(super) fn bearer_token(value: &str) -> Option<&str> {
    let mut parts = value.split_ascii_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    parts.next()
}

pub(super) fn peer_json_matches(
    value: &serde_json::Value,
    matches: &impl Fn(&str) -> bool,
) -> bool {
    match value {
        serde_json::Value::String(text) => matches(text),
        serde_json::Value::Object(entries) => entries
            .iter()
            .any(|(key, value)| matches(key) || peer_json_matches(value, matches)),
        serde_json::Value::Array(entries) => entries
            .iter()
            .any(|value| peer_json_matches(value, matches)),
        serde_json::Value::Number(number) => matches(&number.to_string()),
        serde_json::Value::Bool(value) => matches(if *value { "true" } else { "false" }),
        serde_json::Value::Null => matches("null"),
    }
}

pub(super) fn check_tool_client_grants(
    tool: &super::CanonicalMcpTool,
    credentials: &SourceCredentials,
) -> Result<(), super::McpError> {
    if !credentials.client_owned {
        return Ok(());
    }
    let grants = credentials
        .resolve()
        .map_err(super::McpError::Configuration)?;
    if grants.headers.is_empty() && grants.templates.is_empty() {
        return Ok(());
    }
    let contains_grant = |text: &str| {
        grants
            .headers
            .values()
            .chain(grants.templates.values())
            .any(|value| {
                let value = value.expose();
                text.contains(value)
                    || bearer_token(value).is_some_and(|token| text.contains(token))
            })
    };
    if contains_grant(&tool.name)
        || contains_grant(&tool.description)
        || peer_json_matches(&tool.input_schema, &contains_grant)
        || tool
            .output_schema
            .as_ref()
            .is_some_and(|schema| peer_json_matches(schema, &contains_grant))
    {
        return Err(super::McpError::Discovery(
            "MCP tool declaration contains a Client credential".into(),
        ));
    }
    Ok(())
}

pub(super) fn peer_json(value: &mut serde_json::Value, credentials: &SourceCredentials) {
    match value {
        serde_json::Value::String(text) => *text = credentials.redact(text),
        serde_json::Value::Array(values) => {
            for value in values {
                peer_json(value, credentials);
            }
        }
        serde_json::Value::Object(values) => {
            let old = std::mem::take(values);
            for (key, mut value) in old {
                peer_json(&mut value, credentials);
                values.insert(credentials.redact(&key), value);
            }
        }
        _ => {}
    }
}

pub(super) fn protocol_error(mut error: ErrorData, credentials: &SourceCredentials) -> ErrorData {
    error.message = credentials.redact(&error.message).into();
    if let Some(data) = &mut error.data {
        peer_json(data, credentials);
    }
    error
}

/// Decoder Display can already contain escaped peer values. Location and
/// category are native facts and do not disclose those values a second time.
pub(super) fn decode_error(error: &serde_json::Error) -> String {
    format!(
        "{:?} at line {} column {}",
        error.classify(),
        error.line(),
        error.column()
    )
}

pub(super) fn service_error(error: &ServiceError, credentials: &SourceCredentials) -> String {
    let diagnostic = match error {
        ServiceError::McpError(error) => {
            format!("Mcp error: {}", protocol_error(error.clone(), credentials))
        }
        ServiceError::TransportSend(error) => format!(
            "Transport send error: Transport [{}] failed",
            error.transport_name
        ),
        ServiceError::Cancelled { reason } => format!(
            "task cancelled for reason {}",
            reason
                .as_deref()
                .map_or_else(|| "<unknown>".into(), |reason| credentials.redact(reason))
        ),
        // These variants contain native facts, never peer text.
        ServiceError::TransportClosed
        | ServiceError::UnexpectedResponse
        | ServiceError::SubscriptionLagged { .. }
        | ServiceError::Timeout { .. }
        | ServiceError::InputRequiredRoundsExceeded { .. } => error.to_string(),
        _ => "unrecognized MCP service error".into(),
    };
    super::bound_error(&diagnostic)
}

pub(super) fn initialize_error(
    error: &rmcp::service::ClientInitializeError,
    credentials: &SourceCredentials,
) -> String {
    use rmcp::service::ClientInitializeError as Error;
    let diagnostic = match error {
        Error::JsonRpcError(error) => format!(
            "JSON-RPC error: {}",
            protocol_error(error.clone(), credentials)
        ),
        Error::TransportError { error, context } => format!(
            "Send message error: Transport [{}] failed when {context}",
            error.transport_name
        ),
        Error::NoCompatibleProtocolVersion {
            client_supported,
            server_supported,
        } => format!(
            "rustX speaks {}; the server speaks {}",
            super::connect::joined_versions(client_supported),
            if server_supported.is_empty() {
                "no revision it disclosed".into()
            } else {
                server_supported
                    .iter()
                    .map(|version| credentials.redact(&version.to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ),
        Error::ExpectedInitResponse(_) => {
            "expected MCP initialized response; received an unexpected message".into()
        }
        Error::ExpectedInitResult(_) => {
            "expected MCP initialized result; received an unexpected result".into()
        }
        Error::ConflictInitResponseId(expected, received)
        | Error::UncorrelatedErrorResponse { expected, received } => {
            let mut received = serde_json::to_value(received).expect("request ID is JSON");
            peer_json(&mut received, credentials);
            format!("uncorrelated MCP initialize response: expected id {expected}, got {received}")
        }
        // ConnectionClosed carries the SDK's local handshake phase.
        Error::ConnectionClosed(_)
        | Error::NoPreferredProtocolVersion
        | Error::Cancelled
        | Error::LegacyFallbackFailed { .. } => error.to_string(),
        _ => "unrecognized MCP initialization error".into(),
    };
    super::bound_error(&diagnostic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::catalog::ResolvedCredential;

    #[test]
    fn typed_peer_errors_redact_json_before_escaping_and_preserve_native_facts() {
        let secret = "escaped\"credential\\533";
        let credentials = SourceCredentials {
            literal_redactions: [secret, "e", "1"]
                .into_iter()
                .map(ResolvedCredential::new)
                .collect(),
            ..Default::default()
        };
        let error = serde_json::from_value(serde_json::json!({
            "code":1, "message":secret, "data":{(secret):[secret]}
        }))
        .unwrap();
        let diagnostic = service_error(&ServiceError::McpError(error), &credentials);
        assert!(diagnostic.starts_with("Mcp error: 1: "));
        assert!(!diagnostic.contains("credential"));
        assert!(diagnostic.contains("<redacted>"));
        for error in [
            ServiceError::Timeout {
                timeout: std::time::Duration::from_secs(1),
            },
            ServiceError::SubscriptionLagged { capacity: 1 },
            ServiceError::InputRequiredRoundsExceeded { max_rounds: 1 },
            ServiceError::TransportClosed,
        ] {
            assert_eq!(service_error(&error, &credentials), error.to_string());
        }
    }
}
