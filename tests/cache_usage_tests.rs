//! Provider-reported cache usage must retain subtotals and reporting coverage.

use std::path::PathBuf;

use genai::chat::{ChatMessage, PromptTokensDetails, Usage};
use nerdbot::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
use nerdbot::agent::outcome::AgentOutcome;
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::tools::echo::EchoTool;
use nerdbot::tools::registry::ToolRegistry;
use serde_json::json;

fn context() -> AgentContext {
    let mut ctx = AgentContext::new(
        "test",
        AgentRunMode::Internal {
            reason: "cache usage test".into(),
        },
        "private personality".into(),
        PathBuf::from("/tmp"),
        vec![],
        vec![],
    );
    ctx.messages = vec![ChatMessage::user("private user prompt")];
    ctx
}

fn details(read: Option<i32>, write: Option<i32>) -> PromptTokensDetails {
    PromptTokensDetails {
        cached_tokens: read,
        cache_creation_tokens: write,
        ..Default::default()
    }
}

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(EchoTool).unwrap();
    registry
}

#[tokio::test]
async fn cache_subtotals_cover_every_call_for_final_and_silent_completions() {
    for final_text in ["finished", ""] {
        let provider = FakeProvider::new(vec![
            FakeResponse::tool_call("echo", json!({"message": "first"}))
                .with_token_usage(100, 10)
                .with_prompt_tokens_details(details(Some(60), Some(20))),
            FakeResponse::tool_call("echo", json!({"message": "second"}))
                .with_token_usage(200, 20)
                .with_prompt_tokens_details(details(Some(140), Some(40))),
            FakeResponse::final_text(final_text)
                .with_token_usage(300, 30)
                .with_prompt_tokens_details(details(Some(220), Some(0))),
        ]);
        let result = run_agent(
            &context(),
            &provider,
            &registry(),
            &AgentLoopConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            result.outcome == AgentOutcome::Silent,
            final_text.is_empty()
        );
        assert_eq!(result.metadata.iterations, 3);
        let usage = result.metadata.token_usage;
        assert_eq!(usage.cached_input_tokens, Some(420));
        assert_eq!(usage.cached_input_reported_calls, 3);
        assert_eq!(usage.cache_creation_input_tokens, Some(60));
        assert_eq!(usage.cache_creation_reported_calls, 3);
        // Cache breakdowns are already part of provider accounting, never extra tokens.
        assert_eq!(usage.input_tokens, 600);
        assert_eq!(usage.output_tokens, 60);
        assert_eq!(usage.total_tokens, 660);
    }
}

#[tokio::test]
async fn partial_reporting_keeps_independent_read_and_write_coverage() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "read only"}))
            .with_token_usage(100, 10)
            .with_prompt_tokens_details(details(Some(70), None)),
        FakeResponse::tool_call("echo", json!({"message": "unreported"})).with_token_usage(200, 20),
        FakeResponse::final_text("write only")
            .with_token_usage(300, 30)
            .with_prompt_tokens_details(details(None, Some(50))),
    ]);
    let result = run_agent(
        &context(),
        &provider,
        &registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.metadata.iterations, 3);
    let usage = result.metadata.token_usage;
    assert_eq!(usage.cached_input_tokens, Some(70));
    assert_eq!(usage.cached_input_reported_calls, 1);
    assert_eq!(usage.cache_creation_input_tokens, Some(50));
    assert_eq!(usage.cache_creation_reported_calls, 1);
    assert_eq!(usage.total_tokens, 660);
}

#[tokio::test]
async fn absent_empty_and_unrelated_details_do_not_report_cache_misses() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "no usage"})),
        FakeResponse::tool_call("echo", json!({"message": "no details"})).with_token_usage(100, 10),
        FakeResponse::tool_call("echo", json!({"message": "empty details"}))
            .with_prompt_tokens_details(PromptTokensDetails::default()),
        FakeResponse::final_text("").with_prompt_tokens_details(PromptTokensDetails {
            audio_tokens: Some(5),
            ..Default::default()
        }),
    ]);
    let result = run_agent(
        &context(),
        &provider,
        &registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, AgentOutcome::Silent);
    assert_eq!(result.metadata.iterations, 4);
    let usage = result.metadata.token_usage;
    assert_eq!(usage.cached_input_tokens, None);
    assert_eq!(usage.cached_input_reported_calls, 0);
    assert_eq!(usage.cache_creation_input_tokens, None);
    assert_eq!(usage.cache_creation_reported_calls, 0);
    assert_eq!(usage.total_tokens, 110);
}

#[tokio::test]
async fn explicit_normalized_zeros_are_reported_even_without_input_output_counts() {
    let provider = FakeProvider::new(vec![
        FakeResponse::final_text("done").with_prompt_tokens_details(details(Some(0), Some(0))),
    ]);
    let result = run_agent(
        &context(),
        &provider,
        &registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    let usage = result.metadata.token_usage;
    assert_eq!(usage.cached_input_tokens, Some(0));
    assert_eq!(usage.cached_input_reported_calls, 1);
    assert_eq!(usage.cache_creation_input_tokens, Some(0));
    assert_eq!(usage.cache_creation_reported_calls, 1);
    assert_eq!(usage.total_tokens, 0);
}

#[tokio::test]
async fn genai_deserialized_zeros_remain_unavailable() {
    // Pin the dependency limitation: JSON zero and omitted counters become
    // indistinguishable before NerdBot sees the normalized response.
    let normalized: Usage = serde_json::from_value(json!({
        "prompt_tokens": 100,
        "completion_tokens": 10,
        "prompt_tokens_details": {"cached_tokens": 0, "cache_creation_tokens": 0}
    }))
    .unwrap();
    let details = normalized.prompt_tokens_details.unwrap();
    assert_eq!(details.cached_tokens, None);
    assert_eq!(details.cache_creation_tokens, None);
    let provider = FakeProvider::new(vec![
        FakeResponse::final_text("done")
            .with_token_usage(100, 10)
            .with_prompt_tokens_details(details),
    ]);
    let result = run_agent(
        &context(),
        &provider,
        &registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    let usage = result.metadata.token_usage;
    assert_eq!(usage.cached_input_tokens, None);
    assert_eq!(usage.cached_input_reported_calls, 0);
    assert_eq!(usage.cache_creation_input_tokens, None);
    assert_eq!(usage.cache_creation_reported_calls, 0);
    assert_eq!(usage.total_tokens, 110);
}

#[tokio::test]
async fn openai_compatible_endpoint_forwards_normalized_cache_usage_to_run_metadata() {
    use nerdbot::config::AppConfig;
    use nerdbot::llm::LlmClient;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let mut config = AppConfig::default();
    config.llm.endpoint = Some(format!("{}/v1/", server.uri()));
    config.llm.max_retries = 0;
    let client = LlmClient::from_config(&config).unwrap();
    for (provider_details, expected_read, expected_write) in [
        (
            json!({"cached_tokens": 80, "cache_creation_tokens": 5}),
            Some(80),
            Some(5),
        ),
        (
            json!({"cached_tokens": 0, "cache_creation_tokens": 0}),
            None,
            None,
        ),
        (serde_json::Value::Null, None, None),
        (
            json!({"cached_tokens": -1, "cache_creation_tokens": -5}),
            None,
            None,
        ),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "response-1", "object": "chat.completion", "model": "local-model",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "done"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 999,
                    "prompt_tokens_details": provider_details}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let loop_config = AgentLoopConfig {
            llm_model: "local-model".into(),
            ..Default::default()
        };
        let result = run_agent(&context(), &client, &ToolRegistry::new(), &loop_config)
            .await
            .unwrap();
        let usage = result.metadata.token_usage;
        assert_eq!(usage.cached_input_tokens, expected_read);
        assert_eq!(
            usage.cached_input_reported_calls,
            u32::from(expected_read.is_some())
        );
        assert_eq!(usage.cache_creation_input_tokens, expected_write);
        assert_eq!(
            usage.cache_creation_reported_calls,
            u32::from(expected_write.is_some())
        );
        // Preserve NerdBot's input + output semantics, not the API's total_tokens.
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(usage.total_tokens, 110);
        server.verify().await;
    }
}

#[tokio::test]
async fn malformed_negative_counters_are_unavailable_without_unsigned_underflow() {
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("echo", json!({"message": "negative read"}))
            .with_token_usage(-100, -10)
            .with_prompt_tokens_details(details(Some(i32::MIN), Some(5))),
        FakeResponse::final_text("negative write")
            .with_token_usage(100, 10)
            .with_prompt_tokens_details(details(Some(30), Some(-1))),
    ]);
    let result = run_agent(
        &context(),
        &provider,
        &registry(),
        &AgentLoopConfig::default(),
    )
    .await
    .unwrap();
    let usage = result.metadata.token_usage;
    assert_eq!(usage.cached_input_tokens, Some(30));
    assert_eq!(usage.cached_input_reported_calls, 1);
    assert_eq!(usage.cache_creation_input_tokens, Some(5));
    assert_eq!(usage.cache_creation_reported_calls, 1);
    assert_eq!(usage.input_tokens, 100);
    assert_eq!(usage.output_tokens, 10);
    assert_eq!(usage.total_tokens, 110);
}
