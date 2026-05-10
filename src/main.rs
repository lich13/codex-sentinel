mod codex;
mod config;
mod desktop_control;
mod hooks;
mod lifecycle;
mod recovery;
mod telegram;
mod ui;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codex_sentinel=info,warn".into()),
        )
        .init();

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("--status") | Some("status") => {
            let status = codex::collect_status()?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Some("--recoverable") | Some("recoverable") => {
            let threads = codex::recoverable_threads(10)?;
            println!("{}", serde_json::to_string_pretty(&threads)?);
        }
        Some("--daemon") | Some("daemon") => {
            let cfg = config::load_or_create()?;
            telegram::run_bot(cfg).await?;
        }
        Some("--lifecycle") | Some("lifecycle") => {
            lifecycle::run_lifecycle()?;
        }
        Some("--lifecycle-status") | Some("lifecycle-status") => {
            let status = lifecycle::status()?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Some("--install-launch-agent") | Some("install-launch-agent") => {
            let path = lifecycle::install_launch_agent()?;
            println!("installed launch agent: {}", path.display());
        }
        Some("--continue") | Some("continue") => {
            let cfg = config::load_or_create()?;
            let thread_id = args
                .get(1)
                .cloned()
                .or_else(|| {
                    codex::read_recent_threads(1)
                        .ok()?
                        .first()
                        .map(|t| t.id.clone())
                })
                .expect("missing thread id and no recent thread found");
            let turn_id =
                codex::continue_thread_blocking(&thread_id, &cfg.recovery.continue_prompt)?;
            println!("submitted visible continue {turn_id} on thread {thread_id}");
        }
        Some("--desktop-control-status") | Some("desktop-control-status") => {
            let status = desktop_control::inspect();
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Some("--open-desktop-permissions") | Some("open-desktop-permissions") => {
            desktop_control::open_permission_settings()?;
        }
        Some("--hook-status") | Some("hook-status") => {
            let status = hooks::inspect_hooks()?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Some("--install-hooks") | Some("install-hooks") => {
            let result = hooks::install_hooks()?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--hook-stop") | Some("hook-stop") => {
            hooks::run_stop_hook_from_stdin()?;
        }
        Some("--config") | Some("config") => {
            let cfg = config::load_or_create()?;
            println!("config: {}", config::config_path().display());
            println!("{}", toml::to_string_pretty(&cfg)?);
        }
        Some("--help") | Some("help") => print_help(),
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
        None => ui::run_gui()?,
    }
    Ok(())
}

fn print_help() {
    println!(
        "Codex Sentinel\n\n\
         Commands:\n\
           codex-sentinel              Open desktop status window\n\
           codex-sentinel status       Print JSON status\n\
           codex-sentinel recoverable  Print recoverable recent threads\n\
           codex-sentinel daemon       Run Telegram bot loop\n\
           codex-sentinel lifecycle    Follow Codex.app and manage Sentinel GUI/daemon\n\
           codex-sentinel continue [thread_id]\n\
           codex-sentinel desktop-control-status\n\
           codex-sentinel lifecycle-status\n\
           codex-sentinel install-launch-agent\n\
           codex-sentinel open-desktop-permissions\n\
           codex-sentinel hook-status\n\
           codex-sentinel install-hooks\n\
           codex-sentinel config       Create/show config\n"
    );
}
