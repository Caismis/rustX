//! Built-in Chat Completions request spelling rules. General defaults follow
//! `ZCode`'s OpenAI-compatible path; endpoint-specific rules are deliberately
//! bounded to the exact API/model combinations in its built-in catalog.

use super::catalog::ChatMaxTokensField;

pub(super) fn chat_max_tokens_field(model: &str, base_url: &str) -> ChatMaxTokensField {
    let opencode_go = matches!(
        base_url,
        "https://opencode.ai/zen/go/v1" | "https://opencode.ai/zen/go/v1/"
    );
    if opencode_go
        && matches!(
            model,
            "glm-5.3-flash"
                | "glm-5.3"
                | "glm-5.2"
                | "glm-5.1"
                | "kimi-k3"
                | "kimi-k2.7-code"
                | "kimi-k2.6"
                | "deepseek-v4.1-flash"
                | "deepseek-v4-pro"
                | "deepseek-v4-flash"
                | "deepseek-v4-flash-vision-exp"
                | "mimo-v2.5"
                | "mimo-v2.5-pro"
                | "hy4-preview"
                | "hy3"
        )
    {
        ChatMaxTokensField::MaxTokens
    } else {
        ChatMaxTokensField::MaxCompletionTokens
    }
}
