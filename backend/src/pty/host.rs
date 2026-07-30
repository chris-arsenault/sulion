//! Process-owning PTY core: open the pseudo-terminal, spawn the shell, pump
//! bytes, feed the shadow emulator, supervise the exit.
//!
//! Deliberately free of Postgres and of any manager bookkeeping — this is the
//! half that runs inside the devenv server process. The spec it spawns from is
//! serializable because it crosses the node↔devenv socket.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;

use portable_pty::{CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, watch, Mutex};
use uuid::Uuid;

use crate::emulator::ShadowEmulator;

/// Bytes broadcast channel capacity. One slot ~= one PTY read chunk. At
/// 8 KiB/read a 4096-slot buffer holds ~32 MiB of un-drained backlog.
const BROADCAST_CAPACITY: usize = 4096;

/// Size of each PTY read. Larger = fewer syscalls, smaller = lower latency.
const READ_CHUNK: usize = 8192;

/// Terminal dimension bounds. `vt100` materialises the normal and alternate
/// grids eagerly, so an unbounded size is an allocation the process cannot
/// refuse: `handle_alloc_error` aborts rather than unwinds, which would take
/// every live PTY on the host with it. Applied at the two points every size
/// passes through — `HostedPty::spawn` and the resize task — so no caller can
/// route around them.
const MIN_COLS: u16 = 20;
const MAX_COLS: u16 = 500;
const MIN_ROWS: u16 = 5;
const MAX_ROWS: u16 = 300;

pub fn clamp_pty_size(size: PtySize) -> PtySize {
    PtySize {
        cols: size.cols.clamp(MIN_COLS, MAX_COLS),
        rows: size.rows.clamp(MIN_ROWS, MAX_ROWS),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Everything needed to spawn a shell in a fresh PTY, resolved by the manager
/// (environment policy included) and applied verbatim by the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostSpawnSpec {
    pub id: Uuid,
    pub shell: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub working_dir: PathBuf,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub cols: u16,
    pub rows: u16,
}

/// How a hosted shell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtyExit {
    pub exit_code: Option<i32>,
}

/// A live shell in a PTY this process owns.
pub struct HostedPty {
    pub id: Uuid,
    /// Broadcast channel of PTY output bytes. Every subscriber gets a copy.
    /// The shadow emulator is fed directly by the reader task, not through
    /// this channel, so it stays current with no subscribers attached.
    pub output: broadcast::Sender<Vec<u8>>,
    /// Inbound-to-PTY mpsc, drained by the writer task.
    pub input: mpsc::Sender<Vec<u8>>,
    /// Resize requests, drained by a small task that calls TIOCSWINSZ.
    pub resize: mpsc::Sender<PtySize>,
    /// Shadow terminal emulator, fed every byte read from the PTY.
    pub emulator: ShadowEmulator,
    /// Process ID of the shell (for signaling). None once reaped.
    pub pid: Arc<std::sync::Mutex<Option<u32>>>,
    exit: watch::Receiver<Option<PtyExit>>,
    /// True once the reader has hit EOF: every byte the shell ever wrote has
    /// been broadcast. The exit watch can fire before this — `child.wait()`
    /// returns at process death while the final output is still in the
    /// kernel's PTY buffer.
    output_done: watch::Receiver<bool>,
    /// The receiver that existed before the shell did, so the first consumer
    /// can observe output from the very first byte. A subscription made
    /// after `spawn` returns races the reader task and can miss the shell's
    /// opening output.
    initial_output: std::sync::Mutex<Option<broadcast::Receiver<Vec<u8>>>>,
}

impl HostedPty {
    pub fn spawn(spec: HostSpawnSpec) -> anyhow::Result<Self> {
        let size = clamp_pty_size(PtySize {
            rows: spec.rows,
            cols: spec.cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(size)
            .map_err(|e| anyhow::anyhow!("openpty failed: {e}"))?;

        let mut cmd = CommandBuilder::new(&spec.shell);
        for arg in &spec.args {
            cmd.arg(arg);
        }
        cmd.cwd(&spec.working_dir);
        cmd.env_clear();
        for (key, value) in &spec.env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| anyhow::anyhow!("spawn shell: {e}"))?;
        // Drop the slave half in the parent; the child owns it now.
        drop(pair.slave);

        let pid = Arc::new(std::sync::Mutex::new(child.process_id()));

        let (out_tx, out_rx) = broadcast::channel::<Vec<u8>>(BROADCAST_CAPACITY);
        let (in_tx, in_rx) = mpsc::channel::<Vec<u8>>(64);
        let (resize_tx, resize_rx) = mpsc::channel::<PtySize>(16);
        let emulator = ShadowEmulator::new(size.rows, size.cols);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| anyhow::anyhow!("clone reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| anyhow::anyhow!("take writer: {e}"))?;
        // The master must outlive the reader/writer tasks so that the
        // kernel keeps the pty alive. Wrap it in an Arc<Mutex> so the
        // resize task can call resize() on it.
        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

        let (done_tx, done_rx) = watch::channel(false);
        spawn_reader_task(spec.id, reader, out_tx.clone(), emulator.clone(), done_tx);
        spawn_writer_task(spec.id, writer, in_rx);
        spawn_resize_task(spec.id, master, emulator.clone(), resize_rx);

        let (exit_tx, exit_rx) = watch::channel(None);
        spawn_supervisor_task(child, pid.clone(), exit_tx);

        Ok(Self {
            id: spec.id,
            output: out_tx,
            input: in_tx,
            resize: resize_tx,
            emulator,
            pid,
            exit: exit_rx,
            output_done: done_rx,
            initial_output: std::sync::Mutex::new(Some(out_rx)),
        })
    }

    /// Resolves true once every byte the shell wrote has been broadcast.
    pub fn output_done(&self) -> watch::Receiver<bool> {
        self.output_done.clone()
    }

    /// The from-birth output subscription, available once. Later consumers
    /// subscribe fresh and rely on the emulator snapshot for what they
    /// missed.
    pub fn take_initial_output(&self) -> Option<broadcast::Receiver<Vec<u8>>> {
        self.initial_output.lock().unwrap().take()
    }

    /// Resolves once the shell has exited and been reaped.
    pub fn exit_watch(&self) -> watch::Receiver<Option<PtyExit>> {
        self.exit.clone()
    }

    /// SIGTERM → grace period → SIGKILL. The supervisor task observes the
    /// resulting exit and fires the exit watch as for any other death.
    pub async fn kill(&self) {
        let maybe_pid = { *self.pid.lock().unwrap() };
        if let Some(pid) = maybe_pid {
            kill_with_grace(pid).await;
        }
    }
}

/// SIGTERM first; if the process is still alive after 3 s, SIGKILL.
pub async fn kill_with_grace(pid: u32) {
    unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    let alive = wait_for_exit(pid, std::time::Duration::from_secs(3)).await;
    if alive {
        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    }
}

// ─── task spawners ───────────────────────────────────────────────────────

fn spawn_reader_task(
    id: Uuid,
    mut reader: Box<dyn Read + Send>,
    tx: broadcast::Sender<Vec<u8>>,
    emulator: ShadowEmulator,
    done: watch::Sender<bool>,
) {
    tokio::task::spawn_blocking(move || {
        let mut buf = [0u8; READ_CHUNK];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: child closed its end of the PTY
                Ok(n) => {
                    let chunk = &buf[..n];
                    // Feed the shadow emulator unconditionally so snapshot-on-attach
                    // stays current even when no clients are subscribed.
                    emulator.process(chunk);
                    let _ = tx.send(chunk.to_vec());
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    tracing::debug!(%id, err = %e, "pty read ended");
                    break;
                }
            }
        }
        let _ = done.send(true);
    });
}

fn spawn_writer_task(id: Uuid, mut writer: Box<dyn Write + Send>, mut rx: mpsc::Receiver<Vec<u8>>) {
    tokio::task::spawn_blocking(move || {
        while let Some(bytes) = rx.blocking_recv() {
            if let Err(err) = writer.write_all(&bytes) {
                tracing::debug!(%id, %err, "pty write failed");
                break;
            }
            let _ = writer.flush();
        }
    });
}

fn spawn_resize_task(
    id: Uuid,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    emulator: ShadowEmulator,
    mut rx: mpsc::Receiver<PtySize>,
) {
    tokio::spawn(async move {
        while let Some(size) = rx.recv().await {
            let size = clamp_pty_size(size);
            {
                let m = master.lock().await;
                if let Err(err) = m.resize(size) {
                    tracing::warn!(%id, %err, "pty resize failed");
                }
            }
            // Keep the emulator dimensions in sync so the next snapshot
            // is correctly shaped.
            emulator.resize(size.rows, size.cols);
        }
    });
}

fn spawn_supervisor_task(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    pid: Arc<std::sync::Mutex<Option<u32>>>,
    exit_tx: watch::Sender<Option<PtyExit>>,
) {
    tokio::task::spawn_blocking(move || {
        let status = child.wait();
        // ExitCode is u32; signal-terminated processes encode the signal
        // differently across impls — we just cast to i32 for storage.
        let exit_code = status.ok().map(|s| s.exit_code() as i32);
        *pid.lock().unwrap() = None;
        let _ = exit_tx.send(Some(PtyExit { exit_code }));
    });
}

async fn wait_for_exit(pid: u32, timeout: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        // Sending signal 0 probes whether the pid is alive without harming it.
        let rc = unsafe { libc::kill(pid as i32, 0) };
        if rc != 0 {
            // errno ESRCH = no such process → already exited
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // Still alive at deadline.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: u16, rows: u16) -> PtySize {
        PtySize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    #[test]
    fn clamps_hostile_dimensions() {
        // A client can send this on an open terminal socket. Unclamped it is
        // ~275 GB of eager vt100 grid allocation, which aborts the process.
        let clamped = clamp_pty_size(size(u16::MAX, u16::MAX));
        assert_eq!(clamped.cols, MAX_COLS);
        assert_eq!(clamped.rows, MAX_ROWS);
    }

    #[test]
    fn clamps_degenerate_dimensions() {
        let clamped = clamp_pty_size(size(0, 0));
        assert_eq!(clamped.cols, MIN_COLS);
        assert_eq!(clamped.rows, MIN_ROWS);
    }

    #[test]
    fn leaves_ordinary_dimensions_alone() {
        let clamped = clamp_pty_size(size(120, 30));
        assert_eq!(clamped.cols, 120);
        assert_eq!(clamped.rows, 30);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hosts_a_shell_without_a_database() {
        let spec = HostSpawnSpec {
            id: Uuid::new_v4(),
            shell: crate::pty::default_shell(),
            args: vec!["-c".into(), "echo hosted-marker".into()],
            working_dir: PathBuf::from("/"),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cols: 80,
            rows: 24,
        };
        let hosted = HostedPty::spawn(spec).expect("spawn");
        let mut output = hosted.take_initial_output().expect("first take");
        assert!(
            hosted.take_initial_output().is_none(),
            "only available once"
        );
        let mut exit = hosted.exit_watch();

        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !String::from_utf8_lossy(&seen).contains("hosted-marker") {
            let chunk = tokio::time::timeout_at(deadline, output.recv())
                .await
                .expect("output before deadline");
            match chunk {
                Ok(bytes) => seen.extend_from_slice(&bytes),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        assert!(String::from_utf8_lossy(&seen).contains("hosted-marker"));

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if exit.borrow().is_some() {
                    break;
                }
                if exit.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .expect("exit watch fires");
        assert!(exit.borrow().is_some());
    }
}
