//! Node-side link to the devenv server.
//!
//! The node listens on a unix socket on the shared run volume; the devenv
//! dials in (reconnecting is the devenv's job). The link keeps a per-session
//! output broadcast the rest of the node subscribes to, correlates
//! spawn/snapshot RPCs on reply ids, and surfaces connection and exit events
//! for the manager to apply to its bookkeeping.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use uuid::Uuid;

use crate::pty::host::HostSpawnSpec;

use super::protocol::{
    decode_bytes, decode_line, encode_bytes, encode_line, DevenvToNode, NodeToDevenv,
};

/// Mirrors the PTY host's broadcast capacity: the node-side fan-out holds the
/// same backlog the devenv-side ring does.
const OUTPUT_CAPACITY: usize = 4096;
const OUTBOUND_CAPACITY: usize = 1024;
const SPAWN_TIMEOUT: Duration = Duration::from_secs(30);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

/// What the link tells its consumer (the PTY manager).
#[derive(Debug, Clone)]
pub enum LinkEvent {
    /// A devenv (re)connected and announced what it hosts.
    Connected { sessions: Vec<Uuid> },
    /// A hosted shell exited.
    Exited { id: Uuid, exit_code: Option<i32> },
}

#[derive(Default)]
struct LinkState {
    /// Outbound frames to the currently-connected devenv, if any.
    outbound: Option<mpsc::Sender<String>>,
    /// Sessions the connected devenv hosts, each with its node-side fan-out.
    outputs: HashMap<Uuid, broadcast::Sender<Vec<u8>>>,
    pending_spawns: HashMap<Uuid, oneshot::Sender<Result<(), String>>>,
    pending_snapshots: HashMap<Uuid, oneshot::Sender<Vec<u8>>>,
}

pub struct DevenvLink {
    state: Mutex<LinkState>,
    events: mpsc::UnboundedSender<LinkEvent>,
}

impl DevenvLink {
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<LinkEvent>) {
        let (events, events_rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                state: Mutex::new(LinkState::default()),
                events,
            }),
            events_rx,
        )
    }

    /// Listen for devenv connections on the shared run volume. One devenv at
    /// a time is expected; a newer connection supersedes the previous one.
    pub async fn run_listener(self: Arc<Self>, sock_path: PathBuf) -> anyhow::Result<()> {
        if sock_path.exists() {
            let _ = std::fs::remove_file(&sock_path);
        }
        if let Some(parent) = sock_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let listener = UnixListener::bind(&sock_path)?;
        tracing::info!(path = %sock_path.display(), "devenv socket listening");
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let link = self.clone();
                    tokio::spawn(async move { link.handle_connection(stream).await });
                }
                Err(err) => {
                    tracing::warn!(%err, "devenv: accept error");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    /// Drive one devenv connection. Public so tests (and the loopback role)
    /// can wire a link to an in-process `DevenvServer` over any duplex
    /// stream.
    pub async fn handle_connection<S>(self: Arc<Self>, stream: S)
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

        {
            let mut state = self.state.lock().await;
            // A superseded connection's pending RPCs can never answer.
            fail_pending(&mut state);
            state.outbound = Some(outbound_tx.clone());
        }

        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(msg) = decode_line::<DevenvToNode>(&line) else {
                tracing::warn!("node: undecodable line from devenv ignored");
                continue;
            };
            match msg {
                DevenvToNode::Hello { pid, sessions } => {
                    let ids: Vec<Uuid> = sessions.iter().map(|entry| entry.id).collect();
                    {
                        let mut state = self.state.lock().await;
                        // Sessions the devenv no longer hosts are gone —
                        // dropping their fan-out closes every subscriber.
                        state.outputs.retain(|id, _| ids.contains(id));
                        for id in &ids {
                            state
                                .outputs
                                .entry(*id)
                                .or_insert_with(|| broadcast::channel(OUTPUT_CAPACITY).0);
                        }
                    }
                    tracing::info!(devenv_pid = pid, sessions = ids.len(), "devenv connected");
                    let _ = self.events.send(LinkEvent::Connected { sessions: ids });
                }
                DevenvToNode::Output { id, data } => {
                    let Ok(bytes) = decode_bytes(&data) else {
                        continue;
                    };
                    if let Some(sender) = self.state.lock().await.outputs.get(&id) {
                        let _ = sender.send(bytes);
                    }
                }
                DevenvToNode::Snapshot { reply_id, data } => {
                    let waiter = self.state.lock().await.pending_snapshots.remove(&reply_id);
                    if let (Some(waiter), Ok(bytes)) = (waiter, decode_bytes(&data)) {
                        let _ = waiter.send(bytes);
                    }
                }
                DevenvToNode::SpawnResult {
                    reply_id,
                    ok,
                    error,
                } => {
                    let waiter = self.state.lock().await.pending_spawns.remove(&reply_id);
                    if let Some(waiter) = waiter {
                        let result = if ok {
                            Ok(())
                        } else {
                            Err(error.unwrap_or_else(|| "spawn failed".into()))
                        };
                        let _ = waiter.send(result);
                    }
                }
                DevenvToNode::Exited { id, exit_code } => {
                    // Dropping the fan-out closes subscribers, which is how
                    // attached terminals learn the shell is gone.
                    self.state.lock().await.outputs.remove(&id);
                    let _ = self.events.send(LinkEvent::Exited { id, exit_code });
                }
                DevenvToNode::Unknown => {
                    tracing::debug!("node: unknown devenv message kind ignored");
                }
            }
        }

        {
            let mut state = self.state.lock().await;
            // Only clear the connection if a newer one has not replaced it.
            if state
                .outbound
                .as_ref()
                .is_some_and(|tx| tx.same_channel(&outbound_tx))
            {
                state.outbound = None;
            }
            fail_pending(&mut state);
        }
        drop(outbound_tx);
        let _ = writer.await;
    }

    pub async fn connected(&self) -> bool {
        self.state.lock().await.outbound.is_some()
    }

    /// Ids of sessions the connected devenv hosts, from this link's view.
    pub async fn live_ids(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self.state.lock().await.outputs.keys().copied().collect();
        ids.sort();
        ids
    }

    /// The node-side fan-out for a session's output.
    pub async fn output_sender(&self, id: Uuid) -> Option<broadcast::Sender<Vec<u8>>> {
        self.state.lock().await.outputs.get(&id).cloned()
    }

    pub async fn spawn(&self, spec: HostSpawnSpec) -> anyhow::Result<()> {
        let id = spec.id;
        let reply_id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.state.lock().await;
            // Register the fan-out before the wire message so no output
            // frame can arrive ahead of it.
            state
                .outputs
                .entry(id)
                .or_insert_with(|| broadcast::channel(OUTPUT_CAPACITY).0);
            state.pending_spawns.insert(reply_id, tx);
        }
        if let Err(err) = self.send(&NodeToDevenv::Spawn { reply_id, spec }).await {
            let mut state = self.state.lock().await;
            state.pending_spawns.remove(&reply_id);
            state.outputs.remove(&id);
            return Err(err);
        }
        let result = tokio::time::timeout(SPAWN_TIMEOUT, rx).await;
        match result {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(message))) => {
                self.state.lock().await.outputs.remove(&id);
                Err(anyhow::anyhow!("{message}"))
            }
            Ok(Err(_)) | Err(_) => {
                let mut state = self.state.lock().await;
                state.pending_spawns.remove(&reply_id);
                state.outputs.remove(&id);
                Err(anyhow::anyhow!("devenv did not answer the spawn"))
            }
        }
    }

    pub async fn input(&self, id: Uuid, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.send(&NodeToDevenv::Input {
            id,
            data: encode_bytes(&bytes),
        })
        .await
    }

    pub async fn resize(&self, id: Uuid, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.send(&NodeToDevenv::Resize { id, rows, cols }).await
    }

    pub async fn kill(&self, id: Uuid) -> anyhow::Result<()> {
        self.send(&NodeToDevenv::Kill { id }).await
    }

    pub async fn snapshot(&self, id: Uuid) -> anyhow::Result<Vec<u8>> {
        let reply_id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        self.state
            .lock()
            .await
            .pending_snapshots
            .insert(reply_id, tx);
        if let Err(err) = self
            .send(&NodeToDevenv::SnapshotRequest { id, reply_id })
            .await
        {
            self.state.lock().await.pending_snapshots.remove(&reply_id);
            return Err(err);
        }
        match tokio::time::timeout(SNAPSHOT_TIMEOUT, rx).await {
            Ok(Ok(bytes)) => Ok(bytes),
            Ok(Err(_)) | Err(_) => {
                self.state.lock().await.pending_snapshots.remove(&reply_id);
                Err(anyhow::anyhow!("devenv did not answer the snapshot"))
            }
        }
    }

    async fn send(&self, msg: &NodeToDevenv) -> anyhow::Result<()> {
        let line = encode_line(msg)?;
        // Enqueue while holding the state lock: node requests run
        // concurrently, and two Input frames released between "fetch the
        // sender" and "enqueue" can swap — observed as transposed keystrokes
        // in the shell. The writer task drains the queue without this lock,
        // so holding it across the send cannot deadlock.
        let state = self.state.lock().await;
        let outbound = state
            .outbound
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("devenv is not connected"))?;
        outbound
            .send(line)
            .await
            .map_err(|_| anyhow::anyhow!("devenv connection closed"))
    }
}

fn fail_pending(state: &mut LinkState) {
    for (_, waiter) in state.pending_spawns.drain() {
        let _ = waiter.send(Err("devenv connection lost".into()));
    }
    state.pending_snapshots.drain().for_each(drop);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devenv::server::DevenvServer;

    /// Wire a link to an in-process devenv server over a duplex stream,
    /// exactly how the integration harness does it.
    async fn linked_server() -> (
        Arc<DevenvLink>,
        mpsc::UnboundedReceiver<LinkEvent>,
        Arc<DevenvServer>,
    ) {
        let (link, events) = DevenvLink::new();
        let server = Arc::new(DevenvServer::new());
        let (node_side, devenv_side) = tokio::io::duplex(1024 * 1024);
        tokio::spawn(server.clone().serve(devenv_side));
        tokio::spawn(link.clone().handle_connection(node_side));
        (link, events, server)
    }

    async fn wait_connected(events: &mut mpsc::UnboundedReceiver<LinkEvent>) -> Vec<Uuid> {
        loop {
            match tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .expect("event before deadline")
                .expect("events open")
            {
                LinkEvent::Connected { sessions } => return sessions,
                LinkEvent::Exited { .. } => continue,
            }
        }
    }

    fn spec(command: &str) -> HostSpawnSpec {
        HostSpawnSpec {
            id: Uuid::new_v4(),
            shell: crate::pty::default_shell(),
            args: vec!["-c".into(), command.into()],
            working_dir: "/".into(),
            env: vec![("PATH".into(), "/usr/bin:/bin".into())],
            cols: 80,
            rows: 24,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_output_snapshot_and_exit_round_trip() {
        let (link, mut events, _server) = linked_server().await;
        assert!(wait_connected(&mut events).await.is_empty());
        assert!(link.connected().await);

        // The pause keeps the echo from racing this test's subscription: the
        // node-side fan-out drops frames that arrive with no subscriber, by
        // design — an attach recovers them from the snapshot instead.
        let spec = spec("sleep 1; echo link-marker; sleep 30");
        let id = spec.id;
        let mut output = {
            link.spawn(spec).await.expect("spawn");
            link.output_sender(id).await.expect("fan-out").subscribe()
        };

        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !String::from_utf8_lossy(&seen).contains("link-marker") {
            let chunk = tokio::time::timeout_at(deadline, output.recv())
                .await
                .expect("output before deadline");
            match chunk {
                Ok(bytes) => seen.extend_from_slice(&bytes),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => panic!("output closed early"),
            }
        }

        let snapshot = link.snapshot(id).await.expect("snapshot");
        assert!(String::from_utf8_lossy(&snapshot).contains("link-marker"));

        link.kill(id).await.expect("kill frame");
        loop {
            match tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .expect("exit before deadline")
                .expect("events open")
            {
                LinkEvent::Exited { id: got, .. } => {
                    assert_eq!(got, id);
                    break;
                }
                LinkEvent::Connected { .. } => continue,
            }
        }
        assert!(link.output_sender(id).await.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn disconnected_link_refuses_process_operations() {
        let (link, _events) = DevenvLink::new();
        assert!(!link.connected().await);
        let err = link.spawn(spec("true")).await.expect_err("must refuse");
        assert!(err.to_string().contains("not connected"));
        assert!(link.input(Uuid::new_v4(), b"x".to_vec()).await.is_err());
    }
}
