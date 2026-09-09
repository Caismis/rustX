//! Regenerate editor artifacts from authoritative document types.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
    std::fs::create_dir_all(&root)?;
    for (name, schema) in rustx::local_runtime::schemas::generate() {
        std::fs::write(
            root.join(name),
            format!("{}\n", serde_json::to_string_pretty(&schema)?),
        )?;
    }
    Ok(())
}
