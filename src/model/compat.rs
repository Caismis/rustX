//! Built-in protocol defaults, resolved before a model binding is frozen.
//! Like `ZCode`'s site rules, these match the actual endpoint, never the user's
//! provider name. Unknown endpoints keep the protocol defaults.

use super::catalog::{ChatMaxTokensField, ResponsesStorageMode};

fn endpoint(base_url: &str) -> Option<url::Url> {
    url::Url::parse(base_url).ok().filter(|url| {
        url.scheme() == "https"
            && url.port_or_known_default() == Some(443)
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn is_deepseek(url: &url::Url) -> bool {
    url.host_str() == Some("api.deepseek.com")
        && matches!(url.path().trim_end_matches('/'), "" | "/v1")
}

pub(super) fn responses_storage(base_url: &str) -> ResponsesStorageMode {
    if endpoint(base_url).as_ref().is_some_and(is_deepseek) {
        ResponsesStorageMode::Stateless
    } else {
        ResponsesStorageMode::Stored
    }
}

pub(super) fn chat_max_tokens_field(model: &str, base_url: &str) -> ChatMaxTokensField {
    let Some(url) = endpoint(base_url) else {
        return ChatMaxTokensField::MaxCompletionTokens;
    };
    let host = url.host_str().unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    let glm = matches!(host, "api.z.ai" | "open.bigmodel.cn")
        && matches!(path, "/api/paas/v4" | "/api/coding/paas/v4");
    let kimi = matches!(host, "api.moonshot.cn" | "api.moonshot.ai") && path == "/v1"
        || matches!(host, "api.kimi.com" | "api.kimi.ai") && path == "/coding/v1";
    let dashscope = matches!(
        host,
        "coding.dashscope.aliyuncs.com" | "coding-intl.dashscope.aliyuncs.com"
    ) && path == "/v1"
        || (matches!(
            host,
            "dashscope.aliyuncs.com" | "dashscope-intl.aliyuncs.com" | "dashscope-us.aliyuncs.com"
        ) || host.ends_with(".maas.aliyuncs.com"))
            && path == "/compatible-mode/v1";
    if is_deepseek(&url) || glm || kimi || dashscope {
        return ChatMaxTokensField::MaxTokens;
    }
    let opencode_go = host == "opencode.ai" && path == "/zen/go/v1";
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
