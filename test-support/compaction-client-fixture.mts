/** The same real multi-call provider scenario for both client acceptance paths. */
export const compactionPrompt = 'conformance: write the chunked compaction history';
export const compactionSummary = "chunked compaction kept the user's goal";

export function compactionCatalog(providerUrl: string, credentialVariable: string): string {
  return `[providers.fixture]
protocol = "openai_chat_completions"
base_url = "${providerUrl}/v1"
api_key = "$${credentialVariable}"
[models."fixture/chat-model"]
provider = "fixture"
id = "chat-model"
context_window = 256000
max_output_tokens = 1024
capabilities = { input_modalities = ["text"], output_modalities = ["text"] }
compat = { chat_reasoning_replay = "omit" }
[models."fixture/summary-model"]
provider = "fixture"
id = "summary-model"
context_window = 16000
max_output_tokens = 4096
capabilities = { input_modalities = ["text"], output_modalities = ["text"], json_schema_output = true }
compat = { chat_reasoning_replay = "omit" }
default_profile = "thinking"
[models."fixture/summary-model".profiles.thinking]
request_params = '{"reasoning_effort":"medium"}'
`;
}

export const compactionModelFields = 'auxiliary = { mode = "explicit", model = "fixture/summary-model" }\n';
