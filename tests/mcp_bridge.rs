use serde_json::json;
use sv_optimizer::{
    bridge, mcp, planner,
    protocol::{FarmSnapshot, farm_bridge_client::FarmBridgeClient},
    state::AppState,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
fn fixture() -> FarmSnapshot {
    serde_json::from_str(include_str!("../fixtures/spring-demo.json")).unwrap()
}

#[tokio::test]
async fn mcp_tools_are_usable_and_plans_survive_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path(), Some(fixture()), "test".into()).unwrap();
    let response=mcp::request(&state,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}})).await.unwrap();
    assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
    let tools = mcp::request(
        &state,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await
    .unwrap();
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 4);
    let plan = state
        .tool("plan_earnings", json!({"horizon_days":5}))
        .await
        .unwrap();
    let id = plan["plan"]["id"].clone();
    drop(state);
    let state = AppState::open(dir.path(), Some(fixture()), "test".into()).unwrap();
    let progress = state.tool("check_plan_progress", json!({})).await.unwrap();
    assert_eq!(progress["plan_id"], id);
    assert!(
        state
            .tool("plan_earnings", json!({"horizon_days":1.5}))
            .await
            .is_err()
    );
    assert!(
        state
            .tool("get_farm_snapshot", json!({"refresh":"yes"}))
            .await
            .is_err()
    );
    let error=mcp::request(&state,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"unknown","arguments":{}}})).await.unwrap();
    assert_eq!(error["result"]["isError"], true);
}
#[tokio::test]
async fn stored_snapshots_cannot_silently_become_live_plans() {
    let dir = tempfile::tempdir().unwrap();
    {
        let state = AppState::open(dir.path(), Some(fixture()), "test".into()).unwrap();
        drop(state);
    }
    let state = AppState::open(dir.path(), None, "test".into()).unwrap();
    assert!(
        state
            .tool("plan_earnings", json!({}))
            .await
            .unwrap_err()
            .to_string()
            .contains("disconnected")
    );
    let historical = state
        .tool("get_farm_snapshot", json!({"refresh":false}))
        .await
        .unwrap();
    assert_eq!(historical["historical"], true);
}
#[tokio::test]
async fn grpc_requires_token_and_supports_live_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::open(dir.path(), None, "valid-token".into()).unwrap();
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = socket.local_addr().unwrap();
    let server_state = state.clone();
    let server = tokio::spawn(async move { bridge::serve_listener(server_state, socket).await });
    let mut client = FarmBridgeClient::connect(format!("http://{address}"))
        .await
        .unwrap();
    let (_, bad_rx) = mpsc::channel::<FarmSnapshot>(1);
    let rejected = client
        .observe(ReceiverStream::new(bad_rx))
        .await
        .unwrap_err();
    assert_eq!(rejected.code(), tonic::Code::Unauthenticated);
    let (tx, rx) = mpsc::channel::<FarmSnapshot>(2);
    let mut request = tonic::Request::new(ReceiverStream::new(rx));
    request
        .metadata_mut()
        .insert("authorization", "Bearer valid-token".parse().unwrap());
    let mut incoming = client.observe(request).await.unwrap().into_inner();
    let mut s = fixture();
    s.captured_at_unix_ms = chrono::Utc::now().timestamp_millis();
    tx.send(s.clone()).await.unwrap();
    let refresh_state = state.clone();
    let wait =
        tokio::spawn(async move { refresh_state.tool("get_farm_snapshot", json!({})).await });
    let request = incoming.message().await.unwrap().unwrap();
    assert!(!request.request_id.is_empty());
    s.captured_at_unix_ms = chrono::Utc::now().timestamp_millis();
    s.money = 777;
    s.refresh_request_id = request.request_id;
    tx.send(s).await.unwrap();
    let result = wait.await.unwrap().unwrap();
    assert_eq!(result["snapshot"]["money"], 777);
    assert_eq!(result["live_connected"], true);
    server.abort();
}
#[test]
fn progress_detects_missing_crops_and_delayed_harvests() {
    let original = fixture();
    let plan = planner::plan(&original, 28, &Default::default()).unwrap();
    let mut current = original.clone();
    current.day = 2;
    let progress = sv_optimizer::progress::crops(&original, &current, &plan);
    assert!(!progress["crop_differences"].as_array().unwrap().is_empty());
    let seed = plan.days[0]
        .actions
        .iter()
        .find(|a| a.kind == "plant")
        .unwrap();
    let tile = seed.tiles[0];
    current
        .plots
        .iter_mut()
        .find(|p| p.tile.as_ref() == Some(&tile))
        .unwrap()
        .crop = Some(sv_optimizer::protocol::GrowingCrop {
        seed_id: seed.item_id.clone(),
        days_to_harvest: 25,
        dead: false,
    });
    let progress = sv_optimizer::progress::crops(&original, &current, &plan);
    assert!(
        !progress["harvest_date_differences"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
