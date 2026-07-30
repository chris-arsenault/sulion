//! The devenv server: the process that owns PTY masters.
//!
//! Sessions live in this process and survive any number of node connections.
//! `serve` handles one connection at a time: it announces the current
//! inventory, pumps every session's output into the connection, and applies
//! node requests. A dropped connection stops the pumps and nothing else — the
//! shells, their PTYs, and the shadow emulators (architecture invariant 4)
//! carry on untouched, which is the survival property of the whole plan.

use std::collections::HashMap;
use std::sync::Arc;

use portable_pty::PtySize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::pty::host::HostedPty;

use super::protocol::{
    decode_bytes, decode_line, encode_bytes, encode_line, DevenvToNode, InventoryEntry,
    NodeToDevenv,
};

/// Outbound frame queue per connection. Output frames dominate; the broadcast
/// ring behind each session absorbs bursts while the node drains.
const OUTBOUND_CAPACITY: usize = 4096;

pub struct DevenvServer {
    sessions: Mutex<HashMap<Uuid, Arc<HostedPty>>>,
}

impl Default for DevenvServer {
    fn default() -> Self {
        Self::new()
    }
}

impl DevenvServer {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub async fn live_session_ids(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self.sessions.lock().await.keys().copied().collect();
        ids.sort();
        ids
    }

    /// Serve one node connection until it drops. Callers loop over
    /// connections; sessions persist between calls.
    pub async fn serve<S>(self: Arc<Self>, stream: S)
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (read_half, mut write_half) = tokio::io::split(stream);
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<String>(OUTBOUND_CAPACITY);

        let writer = tokio::spawn(async move {
            while let Some(line) = outbound_rx.recv().await {
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        // Hello with inventory, then a pump per existing session.
        let mut snapshot_channels: HashMap<Uuid, mpsc::Sender<Uuid>> = HashMap::new();
        {
            let sessions = self.sessions.lock().await;
            let inventory = {
                let mut ids: Vec<Uuid> = sessions.keys().copied().collect();
                ids.sort();
                ids.into_iter()
                    .map(|id| InventoryEntry { id })
                    .collect::<Vec<_>>()
            };
            if send_frame(
                &outbound_tx,
                &DevenvToNode::Hello {
                    pid: std::process::id(),
                    sessions: inventory,
                },
            )
            .await
            .is_err()
            {
                writer.abort();
                return;
            }
            for (id, pty) in sessions.iter() {
                snapshot_channels.insert(*id, self.clone().start_pump(pty.clone(), &outbound_tx));
            }
        }

        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(msg) = decode_line::<NodeToDevenv>(&line) else {
                tracing::warn!("devenv: undecodable line from node ignored");
                continue;
            };
            match msg {
                NodeToDevenv::Spawn { reply_id, spec } => {
                    if self
                        .handle_spawn(reply_id, spec, &mut snapshot_channels, &outbound_tx)
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                NodeToDevenv::Input { id, data } => {
                    let Ok(bytes) = decode_bytes(&data) else {
                        tracing::warn!(%id, "devenv: undecodable input payload ignored");
                        continue;
                    };
                    if let Some(pty) = self.sessions.lock().await.get(&id).cloned() {
                        let _ = pty.input.send(bytes).await;
                    }
                }
                NodeToDevenv::Resize { id, rows, cols } => {
                    if let Some(pty) = self.sessions.lock().await.get(&id).cloned() {
                        let _ = pty
                            .resize
                            .send(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            })
                            .await;
                    }
                }
                NodeToDevenv::Kill { id } => {
                    if let Some(pty) = self.sessions.lock().await.get(&id).cloned() {
                        // The TERM→grace→KILL ladder takes seconds; never
                        // stall the read loop on it. The exit surfaces
                        // through the pump like any other death.
                        tokio::spawn(async move { pty.kill().await });
                    }
                }
                NodeToDevenv::SnapshotRequest { id, reply_id } => {
                    // Routed through the session's pump so the snapshot frame
                    // is ordered after every output frame already forwarded —
                    // the attach flow depends on that ordering.
                    let known = match snapshot_channels.get(&id) {
                        Some(tx) => tx.send(reply_id).await.is_ok(),
                        None => false,
                    };
                    if !known {
                        // Session gone (or its pump ended): answer with an
                        // empty snapshot rather than leaving the node's RPC
                        // hanging.
                        if send_frame(
                            &outbound_tx,
                            &DevenvToNode::Snapshot {
                                reply_id,
                                data: encode_bytes(&[]),
                            },
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                }
                NodeToDevenv::Unknown => {
                    tracing::debug!("devenv: unknown node message kind ignored");
                }
            }
        }

        drop(outbound_tx);
        let _ = writer.await;
    }

    /// Spawns a shell for the node and answers with a `SpawnResult`. `Err`
    /// means the connection's outbound queue closed.
    async fn handle_spawn(
        self: &Arc<Self>,
        reply_id: Uuid,
        spec: crate::pty::host::HostSpawnSpec,
        snapshot_channels: &mut HashMap<Uuid, mpsc::Sender<Uuid>>,
        outbound: &mpsc::Sender<String>,
    ) -> Result<(), ()> {
        let id = spec.id;
        let mut sessions = self.sessions.lock().await;
        let result = if sessions.contains_key(&id) {
            Err(anyhow::anyhow!("PTY session {id} is already live"))
        } else {
            HostedPty::spawn(spec)
        };
        let (reply, spawned) = match result {
            Ok(pty) => {
                let pty = Arc::new(pty);
                sessions.insert(id, pty.clone());
                (
                    DevenvToNode::SpawnResult {
                        reply_id,
                        ok: true,
                        error: None,
                    },
                    Some(pty),
                )
            }
            Err(err) => (
                DevenvToNode::SpawnResult {
                    reply_id,
                    ok: false,
                    error: Some(err.to_string()),
                },
                None,
            ),
        };
        drop(sessions);
        // Reply before the pump starts: a fast shell's first output must not
        // reach the wire ahead of the spawn result. The from-birth
        // subscription buffers those bytes until the pump drains them.
        send_frame(outbound, &reply).await?;
        if let Some(pty) = spawned {
            snapshot_channels.insert(id, self.clone().start_pump(pty, outbound));
        }
        Ok(())
    }

    /// Per-connection forwarding task for one session: output frames, ordered
    /// snapshots, and the exit event. Ends when the connection's outbound
    /// queue closes or the session exits; the session itself is only removed
    /// from the server on exit.
    fn start_pump(
        self: Arc<Self>,
        pty: Arc<HostedPty>,
        outbound: &mpsc::Sender<String>,
    ) -> mpsc::Sender<Uuid> {
        let (snap_tx, mut snap_rx) = mpsc::channel::<Uuid>(16);
        let outbound = outbound.clone();
        // The spawning connection's pump takes the from-birth subscription so
        // no opening output can be lost to the reader-task race; reconnect
        // pumps subscribe fresh and the snapshot covers what they missed.
        let mut output = pty
            .take_initial_output()
            .unwrap_or_else(|| pty.output.subscribe());
        let mut exit = pty.exit_watch();
        tokio::spawn(async move {
            let id = pty.id;

            loop {
                let already_exited = exit.borrow_and_update().is_some();
                if already_exited {
                    // The process is dead but its last bytes may still be in
                    // the kernel's PTY buffer: forward output until the
                    // reader reports EOF (bounded — orphaned grandchildren
                    // can hold the slave open indefinitely), so the exit
                    // frame never truncates the shell's final output.
                    let mut done = pty.output_done();
                    let flush_deadline =
                        tokio::time::Instant::now() + std::time::Duration::from_secs(2);
                    while !*done.borrow() {
                        tokio::select! {
                            chunk = output.recv() => match chunk {
                                Ok(bytes) => {
                                    let frame = DevenvToNode::Output { id, data: encode_bytes(&bytes) };
                                    if send_frame(&outbound, &frame).await.is_err() {
                                        break;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            },
                            _ = done.changed() => {}
                            _ = tokio::time::sleep_until(flush_deadline) => break,
                        }
                    }
                    drain_pending(&mut output, &outbound, id).await;
                    let exit_code = exit.borrow().map(|e| e.exit_code).unwrap_or(None);
                    let _ = send_frame(&outbound, &DevenvToNode::Exited { id, exit_code }).await;
                    self.sessions.lock().await.remove(&id);
                    break;
                }
                tokio::select! {
                    chunk = output.recv() => match chunk {
                        Ok(bytes) => {
                            let frame = DevenvToNode::Output { id, data: encode_bytes(&bytes) };
                            if send_frame(&outbound, &frame).await.is_err() {
                                break;
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                            tracing::warn!(%id, count, "devenv output pump lagged");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    request = snap_rx.recv() => match request {
                        Some(reply_id) => {
                            // Forward everything already broadcast before
                            // rendering, so the snapshot supersedes exactly
                            // the frames the node has seen.
                            drain_pending(&mut output, &outbound, id).await;
                            let frame = DevenvToNode::Snapshot {
                                reply_id,
                                data: encode_bytes(&pty.emulator.snapshot()),
                            };
                            if send_frame(&outbound, &frame).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                    changed = exit.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        // Loop around: the top of the loop handles the exit.
                    }
                }
            }
        });
        snap_tx
    }
}

async fn drain_pending(
    output: &mut tokio::sync::broadcast::Receiver<Vec<u8>>,
    outbound: &mpsc::Sender<String>,
    id: Uuid,
) {
    loop {
        match output.try_recv() {
            Ok(bytes) => {
                let frame = DevenvToNode::Output {
                    id,
                    data: encode_bytes(&bytes),
                };
                if send_frame(outbound, &frame).await.is_err() {
                    return;
                }
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(_) => return,
        }
    }
}

async fn send_frame(outbound: &mpsc::Sender<String>, frame: &DevenvToNode) -> Result<(), ()> {
    let Ok(line) = encode_line(frame) else {
        return Err(());
    };
    outbound.send(line).await.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty::host::HostSpawnSpec;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};

    struct TestNode {
        lines: tokio::io::Lines<BufReader<ReadHalf<DuplexStream>>>,
        write: WriteHalf<DuplexStream>,
    }

    impl TestNode {
        fn connect(server: &Arc<DevenvServer>) -> Self {
            let (node_side, devenv_side) = tokio::io::duplex(1024 * 1024);
            tokio::spawn(server.clone().serve(devenv_side));
            let (read, write) = tokio::io::split(node_side);
            Self {
                lines: BufReader::new(read).lines(),
                write,
            }
        }

        async fn send(&mut self, msg: &NodeToDevenv) {
            let line = encode_line(msg).unwrap();
            self.write.write_all(line.as_bytes()).await.unwrap();
        }

        async fn next(&mut self) -> DevenvToNode {
            // Generous: the whole unit suite spawns shells in parallel and a
            // loaded machine can hold a frame back for a while.
            let deadline = std::time::Duration::from_secs(30);
            loop {
                let line = tokio::time::timeout(deadline, self.lines.next_line())
                    .await
                    .expect("frame before deadline")
                    .expect("stream readable")
                    .expect("stream open");
                if let Some(frame) = decode_line::<DevenvToNode>(&line) {
                    return frame;
                }
            }
        }
    }

    fn spawn_spec(marker: &str) -> HostSpawnSpec {
        HostSpawnSpec {
            id: Uuid::new_v4(),
            shell: crate::pty::default_shell(),
            args: vec![
                "-c".into(),
                format!("echo {marker}; sleep 30; echo never-reached"),
            ],
            working_dir: "/".into(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cols: 80,
            rows: 24,
        }
    }

    async fn read_output_until(node: &mut TestNode, id: Uuid, needle: &str) {
        let mut seen = Vec::new();
        loop {
            match node.next().await {
                DevenvToNode::Output { id: got, data } if got == id => {
                    seen.extend_from_slice(&decode_bytes(&data).unwrap());
                    if String::from_utf8_lossy(&seen).contains(needle) {
                        return;
                    }
                }
                _ => continue,
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sessions_survive_a_dropped_connection_and_reannounce() {
        let server = Arc::new(DevenvServer::new());

        // First connection: empty inventory, spawn, see output.
        let mut node = TestNode::connect(&server);
        match node.next().await {
            DevenvToNode::Hello { sessions, .. } => assert!(sessions.is_empty()),
            other => panic!("expected hello, got {other:?}"),
        }
        let spec = spawn_spec("survival-marker");
        let session_id = spec.id;
        let reply_id = Uuid::new_v4();
        node.send(&NodeToDevenv::Spawn { reply_id, spec }).await;
        match node.next().await {
            DevenvToNode::SpawnResult {
                reply_id: got,
                ok,
                error,
            } => {
                assert_eq!(got, reply_id);
                assert!(ok, "spawn failed: {error:?}");
            }
            other => panic!("expected spawn result, got {other:?}"),
        }
        read_output_until(&mut node, session_id, "survival-marker").await;

        // The node dies.
        drop(node);

        // A new node connects: the session is still there, and its snapshot
        // holds the pre-drop output — no gap in the shadow emulator.
        let mut node = TestNode::connect(&server);
        match node.next().await {
            DevenvToNode::Hello { sessions, .. } => {
                assert_eq!(
                    sessions.iter().map(|s| s.id).collect::<Vec<_>>(),
                    vec![session_id]
                );
            }
            other => panic!("expected hello, got {other:?}"),
        }
        let snap_reply = Uuid::new_v4();
        node.send(&NodeToDevenv::SnapshotRequest {
            id: session_id,
            reply_id: snap_reply,
        })
        .await;
        loop {
            match node.next().await {
                DevenvToNode::Snapshot { reply_id, data } => {
                    assert_eq!(reply_id, snap_reply);
                    let rendered =
                        String::from_utf8_lossy(&decode_bytes(&data).unwrap()).to_string();
                    assert!(
                        rendered.contains("survival-marker"),
                        "snapshot missing pre-drop output: {rendered:?}"
                    );
                    break;
                }
                _ => continue,
            }
        }

        // Kill through the new connection; the exit comes back and the
        // inventory empties.
        node.send(&NodeToDevenv::Kill { id: session_id }).await;
        loop {
            match node.next().await {
                DevenvToNode::Exited { id, .. } => {
                    assert_eq!(id, session_id);
                    break;
                }
                _ => continue,
            }
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !server.live_session_ids().await.is_empty() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "session not removed after exit"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn input_reaches_the_shell_and_failed_spawns_report() {
        let server = Arc::new(DevenvServer::new());
        let mut node = TestNode::connect(&server);
        let _hello = node.next().await;

        // A shell that echoes stdin back.
        let spec = HostSpawnSpec {
            id: Uuid::new_v4(),
            shell: crate::pty::default_shell(),
            args: vec!["-i".into()],
            working_dir: "/".into(),
            env: vec![
                ("PATH".into(), "/usr/bin:/bin".into()),
                ("PS1".into(), "$ ".into()),
                ("TERM".into(), "xterm-256color".into()),
            ],
            cols: 80,
            rows: 24,
        };
        let id = spec.id;
        node.send(&NodeToDevenv::Spawn {
            reply_id: Uuid::new_v4(),
            spec,
        })
        .await;
        let _spawn_result = node.next().await;
        node.send(&NodeToDevenv::Input {
            id,
            data: encode_bytes(b"echo in-band-marker\n"),
        })
        .await;
        read_output_until(&mut node, id, "in-band-marker").await;

        // Spawning a shell that does not exist fails loudly, not fatally.
        let bad = HostSpawnSpec {
            id: Uuid::new_v4(),
            shell: "/definitely/not/a/shell".into(),
            args: vec![],
            working_dir: "/".into(),
            env: vec![],
            cols: 80,
            rows: 24,
        };
        let reply_id = Uuid::new_v4();
        node.send(&NodeToDevenv::Spawn {
            reply_id,
            spec: bad,
        })
        .await;
        loop {
            match node.next().await {
                DevenvToNode::SpawnResult {
                    reply_id: got,
                    ok,
                    error,
                } if got == reply_id => {
                    assert!(!ok);
                    assert!(error.is_some());
                    break;
                }
                _ => continue,
            }
        }

        node.send(&NodeToDevenv::Kill { id }).await;
        loop {
            if let DevenvToNode::Exited { id: got, .. } = node.next().await {
                if got == id {
                    break;
                }
            }
        }
    }
}
