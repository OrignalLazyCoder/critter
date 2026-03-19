use crate::{core, network, system::paths};

#[cfg(feature = "web")]
use crate::web;

#[cfg_attr(not(feature = "web"), allow(dead_code))]
#[derive(Debug, Clone)]
pub struct CliArgs {
    pub profile: String,
    pub mode: RunMode,
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Web,
    Tui,
    Node,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_args(&args);
    paths::set_profile(cli.profile.clone());

    #[cfg(feature = "web")]
    {
        match cli.mode {
            RunMode::Web => {
                if cli.host != "127.0.0.1" && cli.password.is_none() {
                    return Err("error: non-localhost --host requires --password".into());
                }
                return web::run_web(web::WebOptions {
                    host: cli.host,
                    port: cli.port,
                    password: cli.password,
                });
            }
            RunMode::Node => {
                if cli.host != "127.0.0.1" && cli.password.is_none() {
                    return Err("error: non-localhost --host requires --password".into());
                }
                return network::signaling::run_signaling_node(
                    network::signaling::SignalingNodeOptions {
                        host: cli.host,
                        port: cli.port,
                        password: cli.password,
                    },
                );
            }
            RunMode::Tui => {}
        }
    }

    #[cfg(not(feature = "web"))]
    if matches!(cli.mode, RunMode::Web | RunMode::Node) {
        return Err("web/node mode requires --features web".into());
    }

    core::runtime::run_main()
}

fn parse_args(args: &[String]) -> CliArgs {
    let mut profile = "peer-0".to_string();
    let mut mode = RunMode::Web;
    let mut host = "127.0.0.1".to_string();
    let mut port = 7777_u16;
    let mut port_explicit = false;
    let mut password: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "web" => {
                mode = RunMode::Web;
                i += 1;
            }
            "tui" => {
                mode = RunMode::Tui;
                i += 1;
            }
            "node" => {
                mode = RunMode::Node;
                i += 1;
            }
            "peer" if i + 1 < args.len() => {
                if let Ok(idx) = args[i + 1].parse::<u32>() {
                    profile = format!("peer-{idx}");
                }
                i += 2;
            }
            "--web" => {
                mode = RunMode::Web;
                i += 1;
            }
            "--host" if i + 1 < args.len() => {
                host = args[i + 1].clone();
                i += 2;
            }
            "--port" if i + 1 < args.len() => {
                if let Ok(v) = args[i + 1].parse::<u16>() {
                    port = v;
                    port_explicit = true;
                }
                i += 2;
            }
            "--password" if i + 1 < args.len() => {
                let v = args[i + 1].trim().to_string();
                if !v.is_empty() {
                    password = Some(v);
                }
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !port_explicit {
        port = match mode {
            RunMode::Web => default_web_port_for_profile(&profile),
            RunMode::Node => 8787,
            RunMode::Tui => port,
        };
    }

    CliArgs {
        profile,
        mode,
        host,
        port,
        password,
    }
}

fn default_web_port_for_profile(profile: &str) -> u16 {
    const BASE_PORT: u16 = 7777;
    const MAX_OFFSET: u16 = 999;
    let idx = profile_index(profile).unwrap_or(0).min(MAX_OFFSET as u32) as u16;
    BASE_PORT.saturating_add(idx)
}

fn profile_index(profile: &str) -> Option<u32> {
    let (_, suffix) = profile.split_once('-')?;
    suffix.parse::<u32>().ok()
}
