use anyhow::Result;
use clap::{Parser, Subcommand};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};
use sv_optimizer::{bridge, chat, mcp, planner, protocol::FarmSnapshot, state::AppState};

#[derive(Parser)]
#[command(version, about = "Read-only Stardew Valley crop earnings assistant")]
struct Cli {
    #[arg(long, global = true, env = "SV_OPTIMIZER_DATA_DIR")]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, default_value_t = 52761)]
    port: u16,
    #[arg(
        long,
        global = true,
        help = "Explicit simulation mode using a JSON snapshot; never your live save"
    )]
    fixture: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand)]
enum Commands {
    Mcp,
    Chat {
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,
        #[arg(long)]
        no_model: bool,
    },
    Doctor {
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,
        #[arg(long)]
        hardware_only: bool,
    },
    Plan {
        #[arg(long)]
        horizon_days: Option<u32>,
        #[arg(long)]
        constraints: Option<PathBuf>,
        #[arg(long)]
        details: bool,
        #[arg(long)]
        json: bool,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let directory = cli.data_dir.unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from(".sv-optimizer"))
            .join("sv-optimizer")
    });
    match cli.command {
        Commands::Doctor {
            model,
            ollama_url,
            hardware_only,
        } => {
            let result = if hardware_only {
                chat::hardware()
            } else {
                chat::doctor(&ollama_url, model.as_deref()).await?
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Chat {
            model,
            ollama_url,
            no_model,
        } => {
            chat::run(
                &directory,
                cli.fixture.as_deref(),
                cli.port,
                &ollama_url,
                model.as_deref(),
                no_model,
            )
            .await?
        }
        Commands::Mcp | Commands::Plan { .. } => {
            let snapshot = cli
                .fixture
                .as_ref()
                .map(|p| {
                    std::fs::read(p).map_err(anyhow::Error::from).and_then(|b| {
                        serde_json::from_slice::<FarmSnapshot>(&b).map_err(anyhow::Error::from)
                    })
                })
                .transpose()?;
            let token = uuid::Uuid::new_v4().to_string();
            let state = AppState::open(&directory, snapshot, token.clone())?;
            let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), cli.port);
            let listener = if !state.fixture {
                // Bind before advertising a fresh token; otherwise a second
                // process could overwrite the config of an active server.
                let socket = tokio::net::TcpListener::bind(address).await?;
                bridge::write_config(&directory, address, &token)?;
                let service = state.clone();
                Some(tokio::spawn(async move {
                    bridge::serve_listener(service, socket).await
                }))
            } else {
                None
            };
            match cli.command {
                Commands::Mcp => {
                    eprintln!(
                        "sv-optimizer MCP ready; {}",
                        if state.fixture {
                            "SIMULATION fixture"
                        } else {
                            "waiting for read-only SMAPI Observer"
                        }
                    );
                    tokio::select! {
                        r=mcp::stdio(state)=>r?,
                        r=async {match listener {Some(l)=>l.await??,None=>std::future::pending::<Result<()>>().await?}Ok::<(),anyhow::Error>(())}=>r?,
                    }
                }
                Commands::Plan {
                    horizon_days,
                    constraints,
                    details,
                    json,
                } => {
                    if !state.fixture {
                        eprintln!(
                            "Waiting for Observer. For daily use prefer `chat` so the bridge stays running."
                        );
                    }
                    if !state.fixture {
                        tokio::time::timeout(std::time::Duration::from_secs(15),async {
                            while state.refresh.read().await.is_none() {tokio::time::sleep(std::time::Duration::from_millis(100)).await;}
                        }).await.map_err(|_|anyhow::anyhow!("Observer did not connect within 15 seconds; start SMAPI with the Observer installed"))?;
                    }
                    let args = serde_json::json!({});
                    let mut args = args;
                    if let Some(h) = horizon_days {
                        args["horizon_days"] = serde_json::json!(h);
                    }
                    if let Some(p) = constraints {
                        args["constraints"] =
                            serde_json::from_slice::<serde_json::Value>(&std::fs::read(p)?)?;
                    }
                    let result = state.tool("plan_earnings", args).await?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&result)?);
                    } else {
                        let p: planner::Plan = serde_json::from_value(result["plan"].clone())?;
                        println!("{}", planner::render(&p, details));
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}
