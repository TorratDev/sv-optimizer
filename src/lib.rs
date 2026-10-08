pub mod bridge;
pub mod chat;
pub mod mcp;
pub mod planner;
pub mod progress;
pub mod state;
pub mod storage;

#[allow(clippy::result_large_err)] // tonic-generated code returns its standard Status type.
pub mod protocol {
    tonic::include_proto!("svoptimizer.v1");
}
