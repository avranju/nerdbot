use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use nerdbot::agent::agent_loop::{AgentContext, AgentLoopConfig, run_agent};
use nerdbot::agent::outcome::AgentOutcome;
use nerdbot::agent::run_mode::AgentRunMode;
use nerdbot::config::{AppConfig, McpServerConfig, McpTransportConfig};
use nerdbot::error::AgentError;
use nerdbot::llm::fake::{FakeProvider, FakeResponse};
use nerdbot::mcp::{McpClientHandle, McpManager, map_call_tool_result};
use nerdbot::tools::registry::ToolRegistry;
use nerdbot::tools::traits::{Tool, ToolContext, ToolOutput};
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, InitializeRequestParams, InitializeResult, ListToolsResult, ServerCapabilities,
    ServerInfo, Tool as RemoteTool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;

#[derive(Clone)]
struct LocalMcpServer {
    handshake_count: Arc<AtomicUsize>,
    initialize_delay: Duration,
}

impl LocalMcpServer {
    fn new(handshake_count: Arc<AtomicUsize>, initialize_delay: Duration) -> Self {
        Self {
            handshake_count,
            initialize_delay,
        }
    }
}

impl ServerHandler for LocalMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("nerdbot-test-server", "1.2.3"))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.handshake_count.fetch_add(1, Ordering::SeqCst);
        if !self.initialize_delay.is_zero() {
            tokio::time::sleep(self.initialize_delay).await;
        }
        context.peer.set_peer_info(request);
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = || {
            Arc::new(
                serde_json::from_value(json!({
                    "type": "object",
                    "properties": {"value": {"type": "string"}}
                }))
                .unwrap(),
            )
        };
        Ok(ListToolsResult::with_all_items(vec![
            RemoteTool::new("echo_mcp", "echo remote input", schema()),
            RemoteTool::new("fail_mcp", "return an MCP error result", schema()),
            RemoteTool::new("slow_mcp", "sleep before returning", schema()),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "echo_mcp" => Ok(CallToolResult::structured(json!({
                "remote": "echo_mcp",
                "arguments": arguments,
            }))
            .into()),
            "fail_mcp" => {
                Ok(CallToolResult::error(vec![ContentBlock::text("remote failure")]).into())
            }
            "slow_mcp" => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(CallToolResult::success(vec![ContentBlock::text("slow done")]).into())
            }
            _ => Err(ErrorData::invalid_params("unknown test tool", None)),
        }
    }
}

async fn spawn_server_on_with_options(
    listener: tokio::net::TcpListener,
    handshake_count: Arc<AtomicUsize>,
    initialize_delay: Duration,
) -> (
    String,
    impl FnOnce(),
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
) {
    let config = StreamableHttpServerConfig::default().with_sse_keep_alive(None);
    let cancellation = config.cancellation_token.clone();
    let server = LocalMcpServer::new(handshake_count.clone(), initialize_delay);
    let service: StreamableHttpService<LocalMcpServer, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(server.clone()), Default::default(), config);
    let router = axum::Router::new().nest_service("/mcp", service);
    let address = listener.local_addr().unwrap();
    let shutdown = cancellation.clone();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await;
    });
    (
        format!("http://{address}/mcp"),
        move || cancellation.cancel(),
        task,
        handshake_count,
    )
}

async fn spawn_server_on(
    listener: tokio::net::TcpListener,
) -> (String, impl FnOnce(), tokio::task::JoinHandle<()>) {
    let (url, stop, task, _) =
        spawn_server_on_with_options(listener, Arc::new(AtomicUsize::new(0)), Duration::ZERO).await;
    (url, stop, task)
}

async fn start_server() -> (String, impl FnOnce()) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let (url, stop, _task) = spawn_server_on(listener).await;
    (url, stop)
}

fn server_config(url: String) -> McpServerConfig {
    McpServerConfig {
        name: "local".into(),
        enabled: true,
        required: true,
        transport: McpTransportConfig::StreamableHttp {
            url,
            bearer_token_env: None,
        },
        tool_prefix: Some("mcp_".into()),
        include_tools: Vec::new(),
        exclude_tools: Vec::new(),
        connect_timeout_secs: 2,
        call_timeout_secs: 1,
    }
}

#[tokio::test]
async fn local_streamable_http_discovery_execution_and_filtering_work() {
    let (url, stop) = start_server().await;
    let mut config = AppConfig::default();
    let mut server = server_config(url);
    server.include_tools = vec!["echo_mcp".into(), "fail_mcp".into()];
    server.exclude_tools = vec!["echo_mcp".into()];
    config.mcp.servers = vec![server];

    let mut registry = ToolRegistry::new();
    let manager = McpManager::initialize(&config, &mut registry)
        .await
        .expect("local MCP server should initialize");
    let specs = registry.specs();
    let names: Vec<_> = specs.iter().map(|tool| tool.name.to_string()).collect();
    assert_eq!(names, vec!["mcp_fail_mcp"]);
    assert_eq!(
        specs[0].description.as_deref(),
        Some("return an MCP error result")
    );
    assert_eq!(
        specs[0].schema.as_ref().unwrap()["properties"]["value"]["type"],
        "string"
    );
    manager.shutdown().await;
    stop();
}

#[tokio::test]
async fn local_proxy_forwards_arguments_and_maps_structured_and_error_results() {
    let (url, stop) = start_server().await;
    let client = McpClientHandle::connect(
        "local",
        url,
        None,
        Duration::from_secs(2),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let tools = client.list_tools(Duration::from_secs(2)).await.unwrap();
    assert_eq!(
        client.peer_implementation().unwrap(),
        ("nerdbot-test-server".into(), "1.2.3".into())
    );
    let echo = tools
        .into_iter()
        .find(|tool| tool.name == "echo_mcp")
        .unwrap();
    let proxy =
        nerdbot::mcp::McpToolProxy::new(client.clone(), echo, "prefixed_echo".into()).unwrap();
    let output = nerdbot::tools::traits::Tool::execute(
        &proxy,
        json!({"value": "hello"}),
        nerdbot::tools::traits::ToolContext {
            run_mode: nerdbot::agent::run_mode::AgentRunMode::Internal {
                reason: "test".into(),
            },
            workspace_root: "/tmp".into(),
            access_policy: nerdbot::channel::ChannelAccessPolicy::allow_all(),
            channel_registry: None,
            pool: None,
            scheduler_notifier: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(output.data["arguments"]["value"], "hello");
    assert!(output.summary.chars().count() <= 4096);

    let failed = client
        .call_tool(CallToolRequestParams::new("fail_mcp"))
        .await
        .unwrap();
    let failed = map_call_tool_result("fail_mcp", failed).unwrap();
    assert!(!failed.success);
    assert_eq!(failed.summary, "remote failure");
    client.shutdown().await;
    stop();
}

#[tokio::test]
async fn stalled_handshake_is_bounded_by_connect_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let router = axum::Router::new()
            .fallback(|| async { std::future::pending::<axum::response::Response>().await });
        let _ = axum::serve(listener, router).await;
    });
    let error = match McpClientHandle::connect(
        "stalled",
        format!("http://{address}/mcp"),
        None,
        Duration::from_millis(20),
        Duration::from_secs(1),
    )
    .await
    {
        Ok(_) => panic!("stalled handshake unexpectedly succeeded"),
        Err(error) => error,
    };
    assert!(matches!(error, AgentError::Mcp(message) if message.contains("timed out")));
    task.abort();
}

#[tokio::test]
async fn transport_loss_reconnects_once_and_retries_the_call() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handshake_count = Arc::new(AtomicUsize::new(0));
    let (url, stop, task, _) =
        spawn_server_on_with_options(listener, handshake_count.clone(), Duration::ZERO).await;
    let client = McpClientHandle::connect(
        "local",
        url,
        None,
        Duration::from_secs(2),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    client.list_tools(Duration::from_secs(2)).await.unwrap();
    assert_eq!(handshake_count.load(Ordering::SeqCst), 1);
    stop();
    task.await.unwrap();

    // The first recovery attempt happens while the endpoint is unavailable. The old connection
    // must remain usable as the recovery marker so a later call can retry recovery.
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );

    let replacement_listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let (_replacement_url, replacement_stop, replacement_task, _) = spawn_server_on_with_options(
        replacement_listener,
        handshake_count.clone(),
        Duration::from_millis(100),
    )
    .await;
    // Both callers arrive while the replacement handshake is in progress. The generation gate
    // must make them share one reconnect and both retry against the new session.
    let (first, second) = tokio::join!(
        client.call_tool(CallToolRequestParams::new("echo_mcp")),
        client.call_tool(CallToolRequestParams::new("echo_mcp")),
    );
    for result in [first, second] {
        let result = result.expect("the client should reconnect to the replacement server");
        assert_eq!(result.structured_content.unwrap()["remote"], "echo_mcp");
    }
    assert_eq!(
        handshake_count.load(Ordering::SeqCst),
        2,
        "the concurrent callers should perform only one replacement handshake"
    );
    replacement_stop();
    replacement_task.await.unwrap();
    client.shutdown().await;
}

#[tokio::test]
async fn optional_and_required_unavailable_servers_have_expected_semantics() {
    let unavailable = "http://127.0.0.1:9/mcp".to_string();
    let mut optional = AppConfig::default();
    let mut server = server_config(unavailable.clone());
    server.required = false;
    optional.mcp.servers = vec![server];
    let mut registry = ToolRegistry::new();
    let manager = McpManager::initialize(&optional, &mut registry)
        .await
        .unwrap();
    assert!(registry.is_empty());
    manager.shutdown().await;

    let mut required = AppConfig::default();
    let mut server = server_config(unavailable);
    server.required = true;
    required.mcp.servers = vec![server];
    let mut registry = ToolRegistry::new();
    assert!(matches!(
        McpManager::initialize(&required, &mut registry).await,
        Err(AgentError::Mcp(_))
    ));
}

#[tokio::test]
async fn slow_mcp_calls_obey_call_timeout() {
    let (url, stop) = start_server().await;
    let client = McpClientHandle::connect(
        "local",
        url,
        None,
        Duration::from_secs(2),
        Duration::from_millis(10),
    )
    .await
    .unwrap();
    client.list_tools(Duration::from_secs(2)).await.unwrap();
    let error = client
        .call_tool(CallToolRequestParams::new("slow_mcp"))
        .await
        .unwrap_err();
    assert!(matches!(error, AgentError::Mcp(message) if message.contains("timed out")));
    client.shutdown().await;
    stop();
}

#[tokio::test]
async fn fake_provider_completes_after_discovered_mcp_tool_call() {
    let (url, stop) = start_server().await;
    let mut config = AppConfig::default();
    config.mcp.servers = vec![server_config(url)];
    let mut registry = ToolRegistry::new();
    let manager = McpManager::initialize(&config, &mut registry)
        .await
        .unwrap();
    let provider = FakeProvider::new(vec![
        FakeResponse::tool_call("mcp_echo_mcp", json!({"value": "from model"})),
        FakeResponse::final_text("finished"),
    ]);
    let context = AgentContext::new(
        "telegram",
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new("456", None),
        },
        "test personality".into(),
        "/tmp".into(),
        vec![],
        vec![],
    );
    let result = run_agent(&context, &provider, &registry, &AgentLoopConfig::default())
        .await
        .unwrap();
    assert!(matches!(result.outcome, AgentOutcome::FinalText(ref text) if text == "finished"));
    assert_eq!(provider.call_count(), 2);
    let second_request = provider.last_request().expect("second model request");
    let tool_result = second_request
        .messages
        .iter()
        .flat_map(|message| message.content.parts())
        .find_map(|part| match part {
            genai::chat::ContentPart::ToolResponse(response) => Some(response.content.clone()),
            _ => None,
        })
        .expect("the MCP result should be returned to the model");
    assert!(tool_result.contains("from model"));
    assert!(tool_result.contains("echo_mcp"));
    manager.shutdown().await;
    stop();
}

#[tokio::test]
async fn denied_address_prevents_discovered_mcp_tool_call() {
    let (url, stop) = start_server().await;
    let mut config = AppConfig::default();
    config.mcp.servers = vec![server_config(url)];
    let mut registry = ToolRegistry::new();
    let manager = McpManager::initialize(&config, &mut registry)
        .await
        .unwrap();
    let provider = FakeProvider::new(vec![FakeResponse::tool_call(
        "mcp_echo_mcp",
        json!({"value": "must not be sent"}),
    )]);
    let context = AgentContext::new(
        "telegram",
        AgentRunMode::InteractiveReply {
            address: nerdbot::channel::ConversationAddress::telegram_chat(123),
            sender: nerdbot::channel::SenderIdentity::new("456", None),
        },
        "test personality".into(),
        "/tmp".into(),
        vec![999],
        vec![],
    );
    assert!(matches!(
        run_agent(&context, &provider, &registry, &AgentLoopConfig::default()).await,
        Err(AgentError::PermissionDenied)
    ));
    assert_eq!(provider.call_count(), 1);
    manager.shutdown().await;
    stop();
}

struct CollisionTool;

#[async_trait::async_trait]
impl Tool for CollisionTool {
    fn name(&self) -> &str {
        "echo_mcp"
    }
    fn description(&self) -> &str {
        "collision"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object"})
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolOutput, AgentError> {
        unreachable!()
    }
}

#[tokio::test]
async fn optional_collision_does_not_partially_register_tools() {
    let (url, stop) = start_server().await;
    let mut config = AppConfig::default();
    let mut server = server_config(url);
    server.required = false;
    server.tool_prefix = Some("".into());
    server.include_tools = vec!["echo_mcp".into()];
    let mut registry = ToolRegistry::new();
    registry.register(CollisionTool).unwrap();
    config.mcp.servers = vec![server];
    let manager = McpManager::initialize(&config, &mut registry)
        .await
        .unwrap();
    assert_eq!(registry.len(), 1);
    manager.shutdown().await;
    stop();
}
