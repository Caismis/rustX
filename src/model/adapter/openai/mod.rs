//! `OpenAI` model adapters (Chat Completions and Responses).
//!
//! All `async-openai` SDK types terminate inside this module: the public
//! surface is the canonical [`ModelAdapter`] trait plus rustX-owned
//! configuration. Automatic retry is bypassed by construction through the
//! no-retry transport in `client.rs`; one adapter invocation performs exactly
//! one provider request attempt.
//!
//! [`ModelAdapter`]: crate::model::adapter::traits::ModelAdapter

pub mod chat_completions;
mod client;
pub mod config;
pub(crate) mod mapping;
pub(crate) mod qwen_xml;
pub mod responses;

pub use chat_completions::OpenAiChatCompletionsAdapter;
pub use config::OpenAiAdapterConfig;
pub use responses::OpenAiResponsesAdapter;

/// `OpenAI` protocols encode request-owned validated PNG bytes as data URLs.
fn image_data_url(
    image: &crate::message::content::ImageReference,
    images: &crate::model::images::ResolvedImages,
) -> Result<String, String> {
    use base64::Engine;
    let bytes = images
        .get(&image.artifact_id)
        .ok_or("unresolved image artifact")?;
    crate::model::images::validate_png(bytes)?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}
