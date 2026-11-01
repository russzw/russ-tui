use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, MutexGuard, mpsc};
use tokio::task;
use tokio::time::sleep;

/// Platform-specific IPC stream type.
#[cfg(unix)]
type IpcStream = tokio::net::UnixStream;

#[cfg(windows)]
type IpcStream = tokio::net::windows::named_pipe::NamedPipeClient;

/// JSON command sent to mpv's IPC.
#[derive(Debug, Serialize)]
struct MpvCommand {
    command: Vec<String>,
}

/// JSON response received from mpv's IPC.
#[derive(Debug, Deserialize)]
struct MpvResponse {
    data: Option<serde_json::Value>,
    error: Option<String>,
}

/// Current playback status polled from mpv.
#[derive(Debug, Clone, Default)]
pub struct PlaybackStatus {
    pub time_pos: Option<f64>,
    pub duration: Option<f64>,
    pub paused: Option<bool>,
    pub media_title: Option<String>,
}

/// Commands the TUI can send to the playback task.
#[derive(Debug)]
pub enum PlayerCommand {
    /// Load a URL, optionally resuming from `start_at` seconds.
    Play {
        url: String,
        start_at: Option<f64>,
    },
    TogglePause,
    Seek {
        seconds: f64,
    },
    Volume {
        percent: f64,
    },
    Quit,
}

/// Handle to the mpv playback backend. Cheap to clone — sends commands via channel.
pub struct MpvPlayer {
    tx: mpsc::UnboundedSender<PlayerCommand>,
    status: Arc<Mutex<PlaybackStatus>>,
}

impl MpvPlayer {
    /// Spawns `mpv` with an IPC socket and returns a player handle plus the background task.
    pub fn new() -> Result<(Self, task::JoinHandle<()>)> {
        let socket_path = default_socket_path();
        let (tx, rx) = mpsc::unbounded_channel();
        let status = Arc::new(Mutex::new(PlaybackStatus::default()));
        let status_clone = status.clone();

        let handle = task::spawn(async move {
            if let Err(e) = run_player_loop(rx, socket_path, status_clone).await {
                eprintln!("mpv player error: {e:#}");
            }
        });

        Ok((Self { tx, status }, handle))
    }

    /// Send a command to the playback task (non-blocking).
    pub fn send(&self, cmd: PlayerCommand) -> Result<()> {
        self.tx.send(cmd).context("player command channel closed")
    }

    /// Read the latest playback status (sync, non-blocking). Returns `None` if the lock is
    /// contended — the caller can just skip this frame's update.
    pub fn status_sync(&self) -> Option<PlaybackStatus> {
        self.status.try_lock().ok().map(|s| s.clone())
    }
}

/// Candidate mpv binaries, tried in order. `RUSS_TUI_MPV` wins, then a PATH
/// lookup, then common Windows install locations (including winget packages).
fn mpv_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(path) = std::env::var("RUSS_TUI_MPV") {
        let path = path.trim();
        if !path.is_empty() {
            candidates.push(PathBuf::from(path));
        }
    }

    candidates.push(PathBuf::from("mpv"));

    #[cfg(windows)]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            candidates.push(local.join("Programs").join("mpv").join("mpv.exe"));
            candidates.push(
                local
                    .join("Microsoft")
                    .join("WinGet")
                    .join("Links")
                    .join("mpv.exe"),
            );

            // Winget portable packages live under a package-specific directory.
            let winget_packages = local.join("Microsoft").join("WinGet").join("Packages");
            if let Ok(entries) = std::fs::read_dir(&winget_packages) {
                for entry in entries.flatten() {
                    let dir = entry.path();
                    let name = dir
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if name.contains("mpv") {
                        if let Some(exe) = find_mpv_in(&dir, 3) {
                            candidates.push(exe);
                        }
                    }
                }
            }
        }

        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(base) = std::env::var(variable) {
                let base = PathBuf::from(base);
                candidates.push(base.join("mpv").join("mpv.exe"));
                candidates.push(base.join("MPV Player").join("mpv.exe"));
            }
        }
        candidates.push(PathBuf::from(r"C:\Program Files\MPV Player\mpv.exe"));
    }

    #[cfg(not(windows))]
    {
        candidates.push(PathBuf::from("/usr/bin/mpv"));
        candidates.push(PathBuf::from("/usr/local/bin/mpv"));
        candidates.push(PathBuf::from("/opt/homebrew/bin/mpv"));
    }

    candidates
}

/// Recursively look for `mpv.exe` under a directory, bounded by `depth`.
#[cfg(windows)]
fn find_mpv_in(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }

    let exe = dir.join("mpv.exe");
    if exe.is_file() {
        return Some(exe);
    }

    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_mpv_in(&path, depth - 1) {
                return Some(found);
            }
        }
    }

    None
}

/// Default IPC socket path. Unix: temp file. Windows: named pipe.
fn default_socket_path() -> PathBuf {
    #[cfg(unix)]
    {
        let mut path = std::env::temp_dir();
        path.push(format!("russ-tui-mpv-{}.sock", std::process::id()));
        path
    }

    #[cfg(windows)]
    {
        PathBuf::from(format!("\\\\.\\pipe\\russ-tui-mpv-{}", std::process::id()))
    }
}

/// Connect to the mpv IPC socket (Unix domain socket or Windows named pipe).
async fn connect_ipc(path: &Path) -> Result<IpcStream> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(path).await?;
        Ok(stream)
    }

    #[cfg(windows)]
    {
        let stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(path)
            .context("failed to connect to mpv named pipe — is mpv running?")?;
        Ok(stream)
    }
}

/// Background task: owns the mpv child process + IPC connection, processes commands.
async fn run_player_loop(
    mut rx: mpsc::UnboundedReceiver<PlayerCommand>,
    socket_path: PathBuf,
    status: Arc<Mutex<PlaybackStatus>>,
) -> Result<()> {
    let socket_arg = socket_path.to_string_lossy().to_string();

    // Spawn mpv with IPC server enabled and idle mode (stays alive between files).
    // Try each candidate binary until one spawns successfully.
    let mut spawn_err: Option<anyhow::Error> = None;
    let mut spawned: Option<tokio::process::Child> = None;

    for candidate in mpv_candidates() {
        match Command::new(&candidate)
            .arg("--input-ipc-server")
            .arg(&socket_arg)
            .arg("--idle=yes")
            .arg("--force-window=immediate")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => {
                spawned = Some(c);
                break;
            }
            Err(e) => {
                spawn_err = Some(anyhow::anyhow!("{}: {e}", candidate.display()));
            }
        }
    }

    let mut child = spawned.ok_or_else(|| {
        anyhow::anyhow!(
            "failed to spawn mpv — install it (winget install shinchiro.mpv) or add it to PATH. Attempts: {}",
            spawn_err.map(|e| format!("{e:#}")).unwrap_or_default()
        )
    })?;

    // Give mpv a moment to create the IPC endpoint.
    sleep(Duration::from_millis(600)).await;

    // Connect to the IPC socket.
    let mut stream = connect_ipc(&socket_path).await?;
    let (read_half, write_half) = tokio::io::split(&mut stream);

    // Wrap halves in Mutex so we can lock them per operation within this single task.
    let read_half = Arc::new(Mutex::new(BufReader::new(read_half)));
    let write_half = Arc::new(Mutex::new(write_half));

    loop {
        // Wait up to 500 ms for a command; on timeout, poll playback status.
        let cmd = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;

        match cmd {
            Ok(Some(cmd)) => {
                let mut w = write_half.lock().await;
                let mut r = read_half.lock().await;
                let should_break = handle_command(&mut w, &mut r, &status, cmd).await?;
                drop(w);
                drop(r);
                if should_break {
                    break;
                }
            }
            Ok(None) => {
                // Channel closed — shutdown.
                break;
            }
            Err(_) => {
                // Timeout — poll status properties.
                let mut w = write_half.lock().await;
                let mut r = read_half.lock().await;
                poll_status(&mut w, &mut r, &status).await?;
                drop(w);
                drop(r);
            }
        }
    }

    // Shut down mpv.
    let _ = child.kill().await;

    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&socket_path);
    }

    Ok(())
}

/// Handle a single player command: send to mpv, read one response line.
async fn handle_command(
    writer: &mut MutexGuard<'_, tokio::io::WriteHalf<&mut IpcStream>>,
    reader: &mut MutexGuard<'_, BufReader<tokio::io::ReadHalf<&mut IpcStream>>>,
    _status: &Arc<Mutex<PlaybackStatus>>,
    cmd: PlayerCommand,
) -> Result<bool> {
    match cmd {
        PlayerCommand::Play { url, start_at } => {
            // Non-fatal: a bad URL shouldn't kill the player task — mpv stays alive.
            let _ = send_recv(writer, reader, &["loadfile", &url]).await;
            // Give mpv a moment to load before seeking (resume support).
            if let Some(pos) = start_at {
                sleep(Duration::from_millis(900)).await;
                let _ = send_recv(writer, reader, &["seek", &pos.to_string(), "absolute"]).await;
            }
        }
        PlayerCommand::TogglePause => {
            send_recv(writer, reader, &["cycle", "pause"]).await?;
        }
        PlayerCommand::Seek { seconds } => {
            send_recv(writer, reader, &["seek", &seconds.to_string(), "relative"]).await?;
        }
        PlayerCommand::Volume { percent } => {
            send_recv(
                writer,
                reader,
                &["set_property", "volume", &percent.to_string()],
            )
            .await?;
        }
        PlayerCommand::Quit => {
            let _ = send_recv(writer, reader, &["quit"]).await;
            return Ok(true); // signal the loop to break
        }
    }
    Ok(false)
}

/// Send one JSON command to mpv and read one response line.
async fn send_recv(
    writer: &mut MutexGuard<'_, tokio::io::WriteHalf<&mut IpcStream>>,
    reader: &mut MutexGuard<'_, BufReader<tokio::io::ReadHalf<&mut IpcStream>>>,
    args: &[&str],
) -> Result<Option<serde_json::Value>> {
    let payload = serde_json::to_string(&MpvCommand {
        command: args.iter().map(|s| s.to_string()).collect(),
    })?;
    writer.write_all(payload.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut line = String::new();
    reader.read_line(&mut line).await?;

    let response: MpvResponse = serde_json::from_str(&line)?;
    match response.error.as_deref() {
        Some("success") | None => Ok(response.data),
        // Normal when idle or between files — treat as "no value" rather than fatal.
        Some("property unavailable") => Ok(None),
        Some(err) => anyhow::bail!("mpv error: {err}"),
    }
}

/// Poll `time-pos`, `duration`, and `pause` from mpv and update shared status.
async fn poll_status(
    writer: &mut MutexGuard<'_, tokio::io::WriteHalf<&mut IpcStream>>,
    reader: &mut MutexGuard<'_, BufReader<tokio::io::ReadHalf<&mut IpcStream>>>,
    status: &Arc<Mutex<PlaybackStatus>>,
) -> Result<()> {
    for prop in ["time-pos", "duration", "pause"] {
        if let Some(data) = send_recv(writer, reader, &["get_property", prop]).await? {
            let mut s = status.lock().await;
            match prop {
                "time-pos" => s.time_pos = data.as_f64(),
                "duration" => s.duration = data.as_f64(),
                "pause" => s.paused = data.as_bool(),
                _ => {}
            }
        }
    }
    Ok(())
}
