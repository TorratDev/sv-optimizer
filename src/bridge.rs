use crate::{
    protocol::{
        FarmSnapshot, RefreshRequest,
        farm_bridge_server::{FarmBridge, FarmBridgeServer},
    },
    state::AppState,
};
use anyhow::{Result, ensure};
use std::{net::SocketAddr, path::Path, pin::Pin};
use tokio::sync::mpsc;
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tonic::{Request, Response, Status, Streaming};

pub struct Bridge {
    state: AppState,
}
#[tonic::async_trait]
impl FarmBridge for Bridge {
    type ObserveStream = Pin<Box<dyn Stream<Item = Result<RefreshRequest, Status>> + Send>>;
    async fn observe(
        &self,
        request: Request<Streaming<FarmSnapshot>>,
    ) -> Result<Response<Self::ObserveStream>, Status> {
        let token = request
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if token != format!("Bearer {}", self.state.token) {
            return Err(Status::unauthenticated("Invalid local session token"));
        }
        let mut incoming = request.into_inner();
        let state = self.state.clone();
        let (tx, rx) = mpsc::channel(16);
        let id = uuid::Uuid::new_v4().to_string();
        {
            let mut refresh = state.refresh.write().await;
            if refresh.is_some() {
                return Err(Status::already_exists("Another Observer is connected"));
            }
            *refresh = Some((id.clone(), tx.clone()));
        }
        tokio::spawn(async move {
            while let Ok(Some(snapshot)) = incoming.message().await {
                if let Err(e) = state.observe(snapshot).await {
                    let _ = tx.send(Err(Status::invalid_argument(e.to_string()))).await;
                    break;
                }
            }
            let mut refresh = state.refresh.write().await;
            if refresh.as_ref().is_some_and(|(session, _)| session == &id) {
                *refresh = None;
            }
            state.updated.notify_waiters();
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }
}
pub async fn serve(state: AppState, address: SocketAddr) -> Result<()> {
    ensure!(
        address.ip().is_loopback(),
        "Bridge must bind to a loopback address"
    );
    tonic::transport::Server::builder()
        .add_service(
            FarmBridgeServer::new(Bridge { state }).max_decoding_message_size(8 * 1024 * 1024),
        )
        .serve(address)
        .await?;
    Ok(())
}
pub async fn serve_listener(state: AppState, listener: tokio::net::TcpListener) -> Result<()> {
    ensure!(
        listener.local_addr()?.ip().is_loopback(),
        "Bridge must bind to loopback"
    );
    tonic::transport::Server::builder()
        .add_service(
            FarmBridgeServer::new(Bridge { state }).max_decoding_message_size(8 * 1024 * 1024),
        )
        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
        .await?;
    Ok(())
}
pub fn write_config(directory: &Path, address: SocketAddr, token: &str) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let config =
        serde_json::json!({"address":format!("http://{address}"),"token":token,"schema_version":1});
    let path = directory.join("bridge.json");
    let temp = directory.join("bridge.json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&config)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))?;
    }
    // On Windows rename does not replace an existing destination. The reader
    // retries during reconnect, so a brief missing-file window is harmless.
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(temp, path)?;
    Ok(())
}
