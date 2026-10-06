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
    call_count: Arc<AtomicUsize>,
}

impl LocalMcpServer {
    fn new(handshake_count: Arc<AtomicUsize>, initialize_delay: Duration) -> Self {
        Self {
            handshake_count,
            initialize_delay,
            call_count: Arc::new(AtomicUsize::new(0)),
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
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let arguments = request.arguments.unwrap_or_default();
        match request.name.as_ref() {
            "echo_mcp" => Ok(CallToolResult::structured(json!({
                "remote": "echo_mcp",
                "arguments": arguments,
            }))
            .into()),
            "protocol_error_mcp" => Err(ErrorData::invalid_params(
                format!(
                    "invalid credential test-secret-token {}",
                    "detail".repeat(8192)
                ),
                Some(json!({"sensitive-body": "test-secret-token", "detail": "x".repeat(65536)})),
            )),
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
async fn transport_loss_recovers_before_the_next_independent_call() {
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

    // The failed call is not replayed. A later, independent call reconnects before dispatch.
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
    // Concurrent independent calls share one replacement handshake under the session lock.
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

#[derive(Clone, Copy)]
enum HttpFault {
    LoseFirstReply,
    ExpireFirstCall,
    StallSlowCall,
    CredentialError,
    CredentialContentType,
}

struct FaultServer {
    url: String,
    calls: Arc<AtomicUsize>,
    handshakes: Arc<AtomicUsize>,
    received_calls: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FaultServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fault_server(fault: HttpFault, initialize_delay: Duration) -> FaultServer {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use axum::middleware::Next;
    use axum::response::IntoResponse;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handshakes = Arc::new(AtomicUsize::new(0));
    let server = LocalMcpServer::new(handshakes.clone(), initialize_delay);
    let calls = server.call_count.clone();
    let service: StreamableHttpService<LocalMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Default::default(),
            StreamableHttpServerConfig::default().with_sse_keep_alive(None),
        );
    let seen = Arc::new(AtomicUsize::new(0));
    let received_calls = seen.clone();
    let router =
        axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn(
                move |request: Request<Body>, next: Next| {
                    let seen = seen.clone();
                    async move {
                        let (parts, body) = request.into_parts();
                        let body = to_bytes(body, usize::MAX).await.unwrap();
                        let json =
                            serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
                        let is_call = json["method"] == "tools/call";
                        let first_call = is_call && seen.fetch_add(1, Ordering::SeqCst) == 0;
                        match fault {
                            HttpFault::CredentialError => {
                                let token = parts
                                    .headers
                                    .get("authorization")
                                    .unwrap()
                                    .to_str()
                                    .unwrap();
                                return (
                                    StatusCode::UNAUTHORIZED,
                                    format!(
                                        "invalid credentials: {token}\n{}",
                                        "sensitive-body".repeat(8192)
                                    ),
                                )
                                    .into_response();
                            }
                            HttpFault::CredentialContentType => {
                                return (
                                    [("content-type", "text/test-secret-token")],
                                    "sensitive-body",
                                )
                                    .into_response();
                            }
                            HttpFault::ExpireFirstCall if first_call => {
                                return StatusCode::NOT_FOUND.into_response();
                            }
                            HttpFault::StallSlowCall
                                if is_call && json["params"]["name"] == "slow_mcp" =>
                            {
                                return std::future::pending().await;
                            }
                            _ => {}
                        }
                        let response = next.run(Request::from_parts(parts, Body::from(body))).await;
                        if matches!(fault, HttpFault::LoseFirstReply) && first_call {
                            // The upstream action has finished, but a proxy loses the successful reply.
                            to_bytes(response.into_body(), usize::MAX).await.unwrap();
                            return (StatusCode::BAD_GATEWAY, "upstream response lost")
                                .into_response();
                        }
                        response
                    }
                },
            ));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    FaultServer {
        url: format!("http://{address}/mcp"),
        calls,
        handshakes,
        received_calls,
        task,
    }
}

async fn fault_client(server: &FaultServer, timeout: Duration) -> Arc<McpClientHandle> {
    McpClientHandle::connect(
        "fault-test",
        server.url.clone(),
        None,
        Duration::from_secs(1),
        timeout,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn lost_response_does_not_replay_a_completed_tool() {
    let server = fault_server(HttpFault::LoseFirstReply, Duration::ZERO).await;
    let client = fault_client(&server, Duration::from_secs(1)).await;
    let error = client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not retried"));
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 1);
    // A new caller may reconnect, but it performs its own action, not a replay.
    client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap();
    assert_eq!(server.calls.load(Ordering::SeqCst), 2);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 2);
    client.shutdown().await;
}

#[tokio::test]
async fn expired_session_does_not_trigger_sdk_tool_replay() {
    let server = fault_server(HttpFault::ExpireFirstCall, Duration::ZERO).await;
    let client = fault_client(&server, Duration::from_secs(1)).await;
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );
    assert_eq!(server.calls.load(Ordering::SeqCst), 0);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 1);
    client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap();
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 2);
    client.shutdown().await;
}

#[tokio::test]
async fn hung_post_is_retired_and_the_next_call_recovers() {
    let server = fault_server(HttpFault::StallSlowCall, Duration::ZERO).await;
    let client = fault_client(&server, Duration::from_millis(100)).await;
    let error = client
        .call_tool(CallToolRequestParams::new("slow_mcp"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    let result = client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap();
    assert_eq!(result.structured_content.unwrap()["remote"], "echo_mcp");
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 2);
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    client.shutdown().await;
}

#[tokio::test]
async fn queued_calls_share_an_overall_deadline() {
    let server = fault_server(HttpFault::StallSlowCall, Duration::ZERO).await;
    let client = fault_client(&server, Duration::from_millis(100)).await;
    let start = std::time::Instant::now();
    let results = futures::future::join_all(
        (0..8).map(|_| client.call_tool(CallToolRequestParams::new("slow_mcp"))),
    )
    .await;
    assert!(results.iter().all(Result::is_err));
    // The previous implementation took eight serial 100ms timeouts. Allow ample
    // scheduling tolerance while proving that queue wait is inside the deadline.
    assert!(
        start.elapsed() < Duration::from_millis(400),
        "elapsed: {:?}",
        start.elapsed()
    );
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 1);
    client.shutdown().await;
}

#[tokio::test]
async fn reconnect_is_inside_the_new_calls_deadline() {
    let server = fault_server(HttpFault::LoseFirstReply, Duration::from_millis(250)).await;
    let client = fault_client(&server, Duration::from_millis(100)).await;
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );
    let start = std::time::Instant::now();
    let error = client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    assert!(start.elapsed() < Duration::from_millis(200));
    assert_eq!(
        server.calls.load(Ordering::SeqCst),
        1,
        "expired calls must not dispatch a tool"
    );
    client.shutdown().await;
}

#[tokio::test]
async fn shutdown_interrupts_reconnect_and_remains_terminal() {
    let server = fault_server(HttpFault::LoseFirstReply, Duration::from_millis(150)).await;
    let client = fault_client(&server, Duration::from_secs(1)).await;
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );
    let calling_client = client.clone();
    let call = tokio::spawn(async move {
        calling_client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.handshakes.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.shutdown().await;
    assert!(
        call.await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("shut down")
    );
    assert!(client.list_tools(Duration::from_secs(1)).await.is_err());
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );
    client.shutdown().await;
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn shutdown_interrupts_a_stalled_call() {
    let server = fault_server(HttpFault::StallSlowCall, Duration::ZERO).await;
    let client = fault_client(&server, Duration::from_secs(1)).await;
    let calling_client = client.clone();
    let call = tokio::spawn(async move {
        calling_client
            .call_tool(CallToolRequestParams::new("slow_mcp"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.received_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    client.shutdown().await;
    assert!(call.await.unwrap().is_err());
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn missing_token_skips_optional_servers_and_fails_required_servers() {
    let env = format!("NERDBOT_MCP_MISSING_{}", uuid::Uuid::new_v4().simple());
    assert!(std::env::var(&env).is_err());
    for required in [false, true] {
        let mut server = server_config("http://127.0.0.1:9/mcp".into());
        server.required = required;
        let McpTransportConfig::StreamableHttp {
            bearer_token_env, ..
        } = &mut server.transport;
        *bearer_token_env = Some(env.clone());
        let mut config = AppConfig::default();
        config.mcp.servers = vec![server];
        let mut registry = ToolRegistry::new();
        match McpManager::initialize(&config, &mut registry).await {
            Ok(manager) => {
                assert!(!required);
                manager.shutdown().await;
            }
            Err(error) => {
                assert!(required);
                assert!(
                    error
                        .to_string()
                        .contains("environment variable is missing")
                );
            }
        }
        assert!(registry.is_empty());
    }
}

#[derive(Clone)]
struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn http_errors_omit_response_bodies_and_configured_credentials_from_logs() {
    use tracing::instrument::WithSubscriber;
    for fault in [HttpFault::CredentialError, HttpFault::CredentialContentType] {
        let server = fault_server(fault, Duration::ZERO).await;
        let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
        let writer = CapturedLogs(buffer.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .finish();
        let error = async {
            let error = match McpClientHandle::connect(
                "credentials",
                server.url.clone(),
                Some("test-secret-token".into()),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .await
            {
                Ok(_) => panic!("unexpectedly connected"),
                Err(error) => error.to_string(),
            };
            // Exercise the same logging boundary as optional-server startup failures.
            tracing::warn!(%error, "startup failure");
            error
        }
        .with_subscriber(subscriber)
        .await;
        if matches!(fault, HttpFault::CredentialError) {
            assert!(error.contains("401"));
        }
        assert!(!error.contains("test-secret-token"));
        assert!(!error.contains("sensitive-body"));
        assert!(error.len() < 2048);
        let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(logs.contains("startup failure"));
        assert!(
            !logs.contains("test-secret-token"),
            "credential leaked: {logs}"
        );
        assert!(!logs.contains("sensitive-body"));
    }
}

#[tokio::test]
async fn protocol_errors_are_redacted_and_do_not_replace_healthy_sessions() {
    let server = fault_server(HttpFault::LoseFirstReply, Duration::ZERO).await;
    let client = McpClientHandle::connect(
        "protocol-test",
        server.url.clone(),
        Some("test-secret-token".into()),
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    // First fault retires the initial session. Recover with an independent call.
    assert!(
        client
            .call_tool(CallToolRequestParams::new("echo_mcp"))
            .await
            .is_err()
    );
    client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap();
    use tracing::instrument::WithSubscriber;
    let buffer = Arc::new(std::sync::Mutex::new(Vec::new()));
    let writer = CapturedLogs(buffer.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let error = async {
        let error = client
            .call_tool(CallToolRequestParams::new("protocol_error_mcp"))
            .await
            .unwrap_err()
            .to_string();
        tracing::warn!(%error, "protocol failure");
        error
    }
    .with_subscriber(subscriber)
    .await;
    assert!(error.contains("[redacted]"));
    assert!(!error.contains("test-secret-token"));
    assert!(error.len() < 2048);
    let logs = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("protocol failure"));
    assert!(!logs.contains("test-secret-token"));
    assert!(!logs.contains("sensitive-body"));
    assert!(!logs.contains(&"detail".repeat(1000)));
    client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap();
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 2);
    client.shutdown().await;
}

#[tokio::test]
async fn cancelling_the_caller_retires_a_pending_transport() {
    let server = fault_server(HttpFault::StallSlowCall, Duration::ZERO).await;
    let client = fault_client(&server, Duration::from_secs(1)).await;
    let calling_client = client.clone();
    let call = tokio::spawn(async move {
        calling_client
            .call_tool(CallToolRequestParams::new("slow_mcp"))
            .await
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.received_calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    call.abort();
    assert!(call.await.unwrap_err().is_cancelled());
    client
        .call_tool(CallToolRequestParams::new("echo_mcp"))
        .await
        .unwrap();
    assert_eq!(server.handshakes.load(Ordering::SeqCst), 2);
    assert_eq!(server.calls.load(Ordering::SeqCst), 1);
    client.shutdown().await;
}
