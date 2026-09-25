//! Rust wire DTOs are the sole input to the public protocol artifacts.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("protocol/app-server");
    std::fs::create_dir_all(&root)?;
    let schema = rustx::app_server::schema::protocol_schema();
    std::fs::write(
        root.join("v22.schema.json"),
        format!("{}\n", serde_json::to_string_pretty(&schema)?),
    )?;
    std::fs::write(
        root.join("fixtures.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&rustx::app_server::schema::fixtures())?
        ),
    )?;
    Ok(())
}
