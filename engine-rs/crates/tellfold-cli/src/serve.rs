//! `tellfold serve` (the Mac app's headless engine) and `tellfold ui` (Tellfold Studio in the browser): the HTTP
//! server of `tellfold-server` behind the options and messages of `cli.py`.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tellfold_runtimes::load_settings;
use tellfold_runtimes::process::kill_live_process_groups;
use tellfold_server::{create_app, log, AppOptions, LlmFactory, ManagerOptions};

use crate::args::Parsed;
use crate::commands::{Done, Exit};
use crate::llm::{fake_mode, make_llm};
use crate::paths::py_resolve;
use crate::ui::Console;

/// `serve` hard-exits this long after stdin closes / after the server stops, if something stuck keeps the process
/// alive. Longer than the worst graceful shutdown (3 s for connections + the manager's 5 s grace + 2 s to reap
/// children), so the force-exit only fires when something is actually stuck.
const SERVE_EOF_EXIT: Duration = Duration::from_secs(15);
const SERVE_SHUTDOWN_EXIT: Duration = Duration::from_secs(5);
/// how long open connections (SSE streams) get to finish before the run manager shuts down
const GRACEFUL: Duration = Duration::from_secs(3);

fn llm_factory() -> LlmFactory {
    Arc::new(|run_dir: Option<&Path>, concurrency: i64, think: bool| make_llm(run_dir, concurrency, think, None))
}

/// `Path.expanduser()`, `~name/...` included.
fn expanduser(path: &Path) -> PathBuf {
    match path.to_str() {
        Some(text) => tellfold_runtimes::settings::expand_user(text),
        None => path.to_path_buf(),
    }
}

/// Kills live agent CLIs and ends the process unless it has exited on its own after `after`.
fn exit_later(after: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(after);
        kill_live_process_groups();
        let _ = std::io::stderr().flush();
        std::process::exit(0);
    });
}

/// Python's `str(OSError)`: `[Errno N] message`.
fn os_error_text(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(code) => format!("[Errno {code}] {}", tellfold_server::upload::strerror(e)),
        None => e.to_string(),
    }
}

/// A listening socket on 127.0.0.1 (SO_REUSEADDR, backlog 128).
fn bind(port: u16) -> std::io::Result<std::net::TcpListener> {
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(true)?;
    socket.bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    let listener = socket.listen(128)?;
    listener.into_std()
}

/// Resolves when Ctrl-C or SIGTERM arrives, or `stdin_closed` fires.
async fn stop_signal(mut stdin_closed: tokio::sync::mpsc::UnboundedReceiver<()>) {
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => futures::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = futures::future::pending::<()>();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate => {}
        _ = stdin_closed.recv() => {}
    }
}

/// Serves `router` until a stop signal, then lets connections finish, shuts the run manager down (which ends the
/// SSE streams) and waits for the server.
async fn run_server(
    listener: std::net::TcpListener,
    router: axum::Router,
    manager: tellfold_server::ManagerThread,
    stdin_closed: tokio::sync::mpsc::UnboundedReceiver<()>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    let listener = tokio::net::TcpListener::from_std(listener)?;
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let mut server = tokio::spawn(async move {
        let mut stop_rx = stop_rx;
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.wait_for(|s| *s).await;
            })
            .await
    });
    tokio::select! {
        _ = stop_signal(stdin_closed) => {}
        _ = &mut server => {}
    }
    let _ = stop_tx.send(true);
    let finished = tokio::time::timeout(GRACEFUL, &mut server).await.is_ok();
    manager.shutdown().await;
    if !finished {
        let _ = tokio::time::timeout(GRACEFUL, &mut server).await;
    }
    Ok(())
}

pub fn serve(_con: &Console, p: &Parsed) -> Done {
    let err = Console::stderr();
    let token = p.str("token").unwrap_or_default();
    if token.chars().count() < 16 {
        err.print("[bold red]error:[/] --token must be at least 16 characters");
        return Err(Exit::Code(2));
    }
    let Some(level) = log::parse_level(&p.str("log_level").unwrap_or_else(|| "warning".into())) else {
        err.print("[bold red]error:[/] --log-level must be debug, info, warning or error");
        return Err(Exit::Code(2));
    };
    log::set_level(level);
    let runs = p.str("runs").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(load_settings(None).runs_dir.filter(|d| !d.is_empty()).unwrap_or_else(|| "runs".into()))
    });
    let runs_dir = py_resolve(&expanduser(&runs));
    if let Err(e) = std::fs::create_dir_all(&runs_dir) {
        err.print(&format!("[bold red]error:[/] cannot create {}: {}", runs_dir.display(), os_error_text(&e)));
        return Err(Exit::Code(1));
    }
    let mut examples_dir = p.str("examples").map(|e| py_resolve(&expanduser(Path::new(&e))));
    if let Some(dir) = examples_dir.as_ref().filter(|d| !d.is_dir()) {
        err.print(&format!(
            "[yellow]warning:[/] --examples {} is not a folder; no examples will be listed",
            dir.display()
        ));
        examples_dir = None;
    }
    let port = p.int("port") as u16;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime");
    let listener = match runtime.block_on(async { bind(port) }) {
        Ok(listener) => listener,
        Err(e) => {
            err.print(&format!("[bold red]error:[/] cannot bind 127.0.0.1:{port}: {}", os_error_text(&e)));
            return Err(Exit::Code(1));
        }
    };
    let bound = listener.local_addr().map(|a| a.port()).unwrap_or(port);
    let fake = fake_mode();
    let result = runtime.block_on(async {
        let mut manager = ManagerOptions::new(&runs_dir, llm_factory());
        manager.examples_dir = examples_dir;
        let (router, manager) =
            create_app(manager, AppOptions { fake_llm: fake, token: Some(token), ..Default::default() });
        err.print_soft(&format!(
            "tellfold serve: 127.0.0.1:{bound}  runs: {}{}",
            runs_dir.display(),
            if fake { "  (fake LLM)" } else { "" }
        ));
        {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{{\"port\": {bound}}}");
            let _ = out.flush();
        }
        let (closed_tx, closed_rx) = tokio::sync::mpsc::unbounded_channel();
        if p.flag("watch_stdin") {
            std::thread::Builder::new()
                .name("tellfold-stdin-watch".into())
                .spawn(move || {
                    let mut stdin = std::io::stdin().lock();
                    let mut buffer = [0u8; 65536];
                    while matches!(stdin.read(&mut buffer), Ok(n) if n > 0) {}
                    let _ = closed_tx.send(());
                    exit_later(SERVE_EOF_EXIT);
                })
                .expect("the stdin watcher thread");
        } else {
            std::mem::forget(closed_tx);
        }
        run_server(listener, router, manager, closed_rx).await
    });
    // A task stuck in a blocking call would otherwise keep the runtime (and the engine behind the app) alive.
    exit_later(SERVE_SHUTDOWN_EXIT);
    runtime.shutdown_timeout(Duration::from_secs(1));
    kill_live_process_groups();
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            err.print(&format!("[bold red]error:[/] {e}"));
            Err(Exit::Code(1))
        }
    }
}

pub fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).is_ok()
}

fn open_browser_later(url: String) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });
}

pub fn ui(con: &Console, p: &Parsed) -> Done {
    let port = p.int("port") as u16;
    if !port_free(port) {
        return Err(crate::commands::fail(
            con,
            &format!("port {port} on 127.0.0.1 is already in use. Pick another with --port."),
        ));
    }
    let runs = PathBuf::from(p.str("runs").unwrap_or_else(|| "runs".into()));
    if let Err(e) = std::fs::create_dir_all(&runs) {
        return Err(crate::commands::fail(con, &crate::paths::py_os_error(&e, &runs)));
    }
    let runs_dir = py_resolve(&runs);
    let fake = fake_mode();
    let url = format!("http://127.0.0.1:{port}/");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a tokio runtime");
    let listener = match runtime.block_on(async { bind(port) }) {
        Ok(listener) => listener,
        Err(e) => return Err(crate::commands::fail(con, &os_error_text(&e))),
    };
    let result = runtime.block_on(async {
        let (router, manager) = create_app(
            ManagerOptions::new(&runs_dir, llm_factory()),
            AppOptions { fake_llm: fake, ..Default::default() },
        );
        con.print_soft(&format!(
            "[bold]Tellfold Studio[/] on [cyan]{url}[/]  runs: {}{}",
            runs_dir.display(),
            if fake { "  [yellow](fake LLM)[/]" } else { "" }
        ));
        con.print("[dim]Press Ctrl-C to quit.[/]");
        if p.flag("open_browser") {
            open_browser_later(url.clone());
        }
        let (_closed_tx, closed_rx) = tokio::sync::mpsc::unbounded_channel();
        run_server(listener, router, manager, closed_rx).await
    });
    runtime.shutdown_timeout(Duration::from_secs(1));
    kill_live_process_groups();
    match result {
        // uvicorn re-raises the Ctrl-C it caught once the server has stopped: click then prints "Aborted."
        Ok(()) => Err(Exit::Abort),
        Err(e) => Err(crate::commands::fail(con, &e.to_string())),
    }
}
