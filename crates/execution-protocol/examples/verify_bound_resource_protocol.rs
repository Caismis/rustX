//! Verify the two bound-resource wire contracts before producing release archives.
use rustx_execution_protocol::ClientRequest;

fn main() -> Result<(), serde_json::Error> {
    // Decode the actual wire fixtures so either half can be reviewed alone.
    // A release needs both the scoped environment and official Skill sync.
    for request in [
        serde_json::json!({"mcp-environment": {
            "service": "docs", "origin": "https://example.test", "names": ["DOCS_TOKEN"]
        }}),
        serde_json::json!({"user-skills": {"digest": {"package": "guide"}}}),
        serde_json::json!({"user-skills": "publish"}),
    ] {
        serde_json::from_value::<ClientRequest>(request)?;
    }
    Ok(())
}
