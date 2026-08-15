//! Live CLI smoke tool for the model plane.
//!
//! This is a developer/example executable, not a model gateway: it
//! instantiates the same production adapters and consumes the common
//! `ModelEvent` interface. It never executes tools, never implements an agent
//! loop, never retries, never persists messages, never starts a server, and
//! never routes among providers automatically.
//!
//! Usage (credentials come from `OPENAI_API_KEY` / `ANTHROPIC_API_KEY`;
//! endpoints come from `RUSTX_OPENAI_BASE_URL` / `RUSTX_ANTHROPIC_BASE_URL`,
//! because no adapter can infer an official endpoint):
//!
//! ```text
//! cargo run --example model_smoke -- --protocol openai-chat --model gpt-5-mini --prompt "Say hello."
//! cargo run --example model_smoke -- --protocol openai-responses --model gpt-5-mini --prompt "Say hello."
//! cargo run --example model_smoke -- --protocol anthropic --model claude-sonnet-4-6 --prompt "Say hello."
//! ```
//!
//! The smoke request carries **no** provider request parameters: reasoning
//! is a model-declared catalog profile, and this example deliberately does
//! not synthesize one, so whatever reaches the wire came from canonical
//! translation alone.

use futures_util::StreamExt;
use rustx::model::{
    AnthropicAdapterConfig, AnthropicMessagesAdapter, ModelAdapter, ModelCapabilities, ModelCompat,
    ModelEvent, ModelInvocationConfig, ModelProtocol, ModelRequest, OpenAiAdapterConfig,
    OpenAiChatCompletionsAdapter, OpenAiResponsesAdapter, RequestParams,
};
use rustx::runtime::CancellationSignal;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (protocol, model, prompt) = parse_args(&args);
    if !matches!(
        protocol.as_str(),
        "openai-chat" | "openai-responses" | "anthropic"
    ) {
        eprintln!(
            "unknown protocol {protocol:?}; expected openai-chat, openai-responses, or anthropic"
        );
        std::process::exit(2);
    }
    let request = build_request(&protocol, &model, &prompt);
    let adapter: Box<dyn ModelAdapter> = match protocol.as_str() {
        "openai-chat" => Box::new(OpenAiChatCompletionsAdapter::new(OpenAiAdapterConfig::new(
            read_secret("OPENAI_API_KEY"),
            base_url("RUSTX_OPENAI_BASE_URL", "https://api.openai.com/v1"),
        ))),
        "openai-responses" => Box::new(OpenAiResponsesAdapter::new(OpenAiAdapterConfig::new(
            read_secret("OPENAI_API_KEY"),
            base_url("RUSTX_OPENAI_BASE_URL", "https://api.openai.com/v1"),
        ))),
        "anthropic" => Box::new(AnthropicMessagesAdapter::new(AnthropicAdapterConfig::new(
            read_secret("ANTHROPIC_API_KEY"),
            base_url("RUSTX_ANTHROPIC_BASE_URL", "https://api.anthropic.com"),
        ))),
        _ => unreachable!("protocol validated above"),
    };

    let runtime = tokio::runtime::Runtime::new().expect("build tokio runtime");
    runtime.block_on(async move {
        let cancellation = CancellationSignal::new();
        let mut stream = adapter.stream(request, cancellation);
        while let Some(event) = stream.next().await {
            match &event {
                ModelEvent::Started => println!("[started]"),
                ModelEvent::TextDelta { text, .. } => print!("{text}"),
                ModelEvent::ReasoningDelta { text, .. } => {
                    println!();
                    println!("[reasoning] {text}");
                }
                ModelEvent::RefusalDelta { text, .. } => {
                    println!();
                    println!("[refusal] {text}");
                }
                ModelEvent::ToolCallStarted { call, .. } => {
                    println!();
                    println!("[tool call] {} ({})", call.name, call.id);
                }
                ModelEvent::ToolCallArgumentsDelta {
                    arguments_delta, ..
                } => print!("{arguments_delta}"),
                ModelEvent::ToolCallCompleted { call, .. } => {
                    println!();
                    println!("[tool completed] {} -> {}", call.name, call.arguments);
                }
                ModelEvent::UsageUpdate { usage } => {
                    println!();
                    println!("[usage] {usage:?}");
                }
                ModelEvent::ContinuationState { .. } => {}
                ModelEvent::Completed {
                    finish_reason,
                    usage,
                } => {
                    println!();
                    println!("[completed] finish_reason={finish_reason:?} usage={usage:?}");
                }
                ModelEvent::Failed { error } => {
                    println!();
                    println!("[failed] {:?}: {}", error.kind, error.message);
                }
            }
        }
    });
}

fn parse_args(args: &[String]) -> (String, String, String) {
    let mut protocol = None;
    let mut model = None;
    let mut prompt = "Say hello.".to_owned();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--protocol" => {
                index += 1;
                protocol = args.get(index).cloned();
            }
            "--model" => {
                index += 1;
                model = args.get(index).cloned();
            }
            "--prompt" => {
                index += 1;
                prompt = args
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| "Say hello.".to_owned());
            }
            other => {
                eprintln!("unknown argument {other:?}");
                std::process::exit(2);
            }
        }
        index += 1;
    }
    let protocol = protocol.unwrap_or_else(|| {
        eprintln!("missing --protocol (openai-chat | openai-responses | anthropic)");
        std::process::exit(2);
    });
    let model = model.unwrap_or_else(|| {
        eprintln!("missing --model");
        std::process::exit(2);
    });
    (protocol, model, prompt)
}

fn build_request(protocol: &str, model: &str, prompt: &str) -> ModelRequest {
    use rustx::message::content::TextBlock;
    use rustx::message::types::{
        InboundKind, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use rustx::runtime::identity::MessageId;
    let protocol = match protocol {
        "openai-chat" => ModelProtocol::OpenAiChatCompletions,
        "openai-responses" => ModelProtocol::OpenAiResponses,
        "anthropic" => ModelProtocol::AnthropicMessages,
        _ => unreachable!("validated in main"),
    };
    ModelRequest {
        invocation: ModelInvocationConfig {
            model: model.to_owned(),
            protocol,
            max_output_tokens: 512,
            request_params: RequestParams::new(),
            capabilities: ModelCapabilities::text_only(true, true),
            compat: ModelCompat::default(),
        },
        messages: vec![MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-smoke-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: prompt.to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })],
        tools: Vec::new(),
        effective_system_prompt: String::new(),
        continuation: None,
    }
}

/// The explicit provider endpoint of a smoke run.
fn base_url(variable: &str, default: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| default.to_owned())
}

fn read_secret(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        eprintln!("missing environment variable {name}");
        std::process::exit(2);
    })
}
