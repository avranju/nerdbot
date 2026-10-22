//! Isolated tracing capture avoids callsite-interest races with unsubscribed parallel runs.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use genai::chat::{ChatMessage, PromptTokensDetails};
use nerdbot::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
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

#[derive(Clone, Default)]
struct CapturedEvents(Arc<Mutex<Vec<serde_json::Map<String, serde_json::Value>>>>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CapturedEvents {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Visitor(serde_json::Map<String, serde_json::Value>);
        impl tracing::field::Visit for Visitor {
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                self.0.insert(field.name().into(), json!(value));
            }

            fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
                self.0.insert(field.name().into(), json!(value));
            }

            fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                self.0.insert(field.name().into(), json!(value));
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0
                    .insert(field.name().into(), json!(format!("{value:?}")));
            }
        }
        let mut visitor = Visitor(Default::default());
        event.record(&mut visitor);
        self.0.lock().unwrap().push(visitor.0);
    }
}

#[tokio::test]
async fn tracing_exposes_numeric_call_and_run_usage_with_reporting_coverage() {
    use tracing::instrument::WithSubscriber;
    use tracing_subscriber::prelude::*;

    for final_text in ["private final answer", ""] {
        let events = CapturedEvents::default();
        let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(events.clone()));
        let provider = FakeProvider::new(vec![
            FakeResponse::tool_call("echo", json!({"message": "tool"}))
                .with_token_usage(100, 10)
                .with_prompt_tokens_details(details(Some(0), Some(20))),
            FakeResponse::final_text(final_text).with_token_usage(200, 20),
        ]);
        let config = AgentLoopConfig {
            llm_model: "requested-model".into(),
            ..Default::default()
        };
        run_agent(&context(), &provider, &registry(), &config)
            .with_subscriber(dispatch.clone())
            .await
            .unwrap();
        let events = events.0.lock().unwrap();
        let calls: Vec<_> = events
            .iter()
            .filter(|e| e["message"] == "LLM call token usage")
            .collect();
        assert_eq!(calls.len(), 2, "captured events: {events:?}");
        assert_eq!(calls[0]["model"], "fake-model");
        assert_eq!(calls[0]["requested_model"], "requested-model");
        assert_eq!(calls[0]["iteration"], 1);
        assert_eq!(calls[0]["input_tokens"], 100);
        assert_eq!(calls[0]["output_tokens"], 10);
        assert_eq!(calls[0]["total_tokens"], 110);
        assert_eq!(calls[0]["cached_input_tokens"], 0);
        assert_eq!(calls[0]["cached_input_reported"], true);
        assert_eq!(calls[0]["cache_creation_input_tokens"], 20);
        assert_eq!(calls[0]["cache_creation_reported"], true);
        assert_eq!(calls[1]["iteration"], 2);
        assert_eq!(calls[1]["cached_input_reported"], false);
        assert_eq!(calls[1]["cache_creation_reported"], false);
        assert!(!calls[1].contains_key("cached_input_tokens"));
        let run = events
            .iter()
            .find(|e| e["message"] == "agent run token usage")
            .unwrap();
        assert_eq!(run["model"], "fake-model");
        assert_eq!(run["iterations"], 2);
        assert_eq!(run["total_tokens"], 330);
        assert_eq!(run["cached_input_tokens"], 0);
        assert_eq!(run["cached_input_reported_calls"], 1);
        assert_eq!(run["cache_creation_input_tokens"], 20);
        assert_eq!(run["cache_creation_reported_calls"], 1);
        let logged = serde_json::to_string(&*events).unwrap();
        for private in [
            "private personality",
            "private user prompt",
            "private final answer",
        ] {
            assert!(!logged.contains(private));
        }
    }
}
