//! Browser acceptance setup only: allocate real immutable bytes through the
//! Artifact owner. Never linked into the product or exposed as a Host API.
use rustx::runtime::identity::ConversationId;
use rustx::tools::artifacts::ArtifactStore;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("expected conversation, fixture store root and source".into());
    }
    let store = ArtifactStore::new(ConversationId::new(&args[0]), &args[1])?;
    let bytes = std::fs::read(&args[2])?;
    if bytes.len() > rustx::tools::artifacts::ARTIFACT_TRANSFER_MAX {
        return Err("fixture exceeds Artifact limit".into());
    }
    let id = store.create_artifact()?;
    {
        let mut writer = store.open_writer(&id)?;
        writer.write_all(&bytes)?;
        writer.flush()?;
    }
    println!("{id}");
    Ok(())
}
