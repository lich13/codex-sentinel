mod app_server_probe;
mod codex;
mod config;
mod control_queue;
mod desktop_control;
mod hooks;
mod lifecycle;
mod maintenance;
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
        Some("--control-worker") | Some("control-worker") => {
            control_queue::run_worker()?;
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
            let result = control_queue::submit_and_wait(control_queue::ControlAction::Continue {
                thread_id,
                prompt: cfg.recovery.continue_prompt,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--append") | Some("append") => {
            let thread_id = args.get(1).cloned().expect("missing thread id for append");
            let prompt = args
                .get(2)
                .map(String::as_str)
                .unwrap_or("继续干。请读取当前状态后开始。");
            let result = control_queue::submit_and_wait(control_queue::ControlAction::Continue {
                thread_id,
                prompt: prompt.to_string(),
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--new") | Some("new") => {
            let prompt = args
                .get(1)
                .map(String::as_str)
                .unwrap_or("继续干。请读取当前状态后开始。");
            let result = control_queue::submit_and_wait(control_queue::ControlAction::NewThread {
                prompt: prompt.to_string(),
                path: None,
            })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--delete") | Some("delete") => {
            let thread_id = args.get(1).cloned().expect("missing thread id for delete");
            let result =
                control_queue::submit_and_wait(control_queue::ControlAction::ArchiveThread {
                    thread_id,
                })?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--clear-archived") | Some("clear-archived") => {
            let result =
                control_queue::submit_and_wait(control_queue::ControlAction::ClearArchived)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--desktop-control-status") | Some("desktop-control-status") => {
            let status = desktop_control::inspect();
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        Some("--debug-new-chat") | Some("debug-new-chat") => {
            desktop_control::prepare_new_thread_visible(None)?;
            println!("opened visible Codex new chat");
        }
        Some("--debug-new-direct") | Some("debug-new-direct") => {
            let prompt = args.get(1).map(String::as_str).unwrap_or(
                "Sentinel debug new thread: please reply with a short acknowledgment only.",
            );
            let result = codex::start_new_thread(prompt, None)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Some("--debug-thread-failure-state") | Some("debug-thread-failure-state") => {
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
            let state = desktop_control::inspect_thread_failure_state(&thread_id)?;
            println!("{state:?}");
        }
        Some("--debug-app-server-thread") | Some("debug-app-server-thread") => {
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
            let probe = app_server_probe::read_thread_probe(&thread_id)?;
            println!("{}", serde_json::to_string_pretty(&probe)?);
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
        Some("--notify-completion") | Some("notify-completion") => {
            hooks::run_notify_completion_from_stdin().await?;
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
           codex-sentinel control-worker Run queued Codex APP control requests\n\
           codex-sentinel lifecycle    Follow Codex.app and manage Sentinel GUI/daemon\n\
           codex-sentinel continue [thread_id]\n\
           codex-sentinel append <thread_id> <prompt>\n\
           codex-sentinel new [prompt]\n\
           codex-sentinel delete <thread_id>\n\
           codex-sentinel clear-archived\n\
           codex-sentinel desktop-control-status\n\
           codex-sentinel debug-new-chat\n\
           codex-sentinel debug-new-direct [prompt]\n\
           codex-sentinel debug-thread-failure-state [thread_id]\n\
           codex-sentinel debug-app-server-thread [thread_id]\n\
           codex-sentinel lifecycle-status\n\
           codex-sentinel install-launch-agent\n\
           codex-sentinel open-desktop-permissions\n\
           codex-sentinel hook-status\n\
           codex-sentinel install-hooks\n\
           codex-sentinel config       Create/show config\n"
    );
}
