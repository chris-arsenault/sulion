//! Node-side link to the devenv servers.
//!
//! The node listens on a unix socket on the shared run volume; devenvs dial
//! in (reconnecting is the devenv's job). Several devenvs are connected at
//! once — one per toolset version — each announcing its identity (container
//! image ID) in its hello; child-mode devenvs announce none and key as
//! [`DEFAULT_IDENT`]. The link routes every session operation to the devenv
//! that hosts the session, keeps a per-session output broadcast the rest of
//! the node subscribes to, and correlates spawn/snapshot RPCs on reply ids.
//! New sessions go to the *current* ident, which the launcher sets from the
//! current toolset image.

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

/// Connection key for devenvs that announce no identity: child mode and
/// pre-versioning containers.
pub const DEFAULT_IDENT: &str = "default";

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
    Connected { ident: String, sessions: Vec<Uuid> },
    /// A hosted shell exited.
    Exited { id: Uuid, exit_code: Option<i32> },
}

#[derive(Default)]
struct LinkState {
    /// Outbound frames per connected devenv, keyed by announced ident.
    connections: HashMap<String, mpsc::Sender<String>>,
    /// Which devenv hosts each session.
    session_host: HashMap<Uuid, String>,
    /// Node-side fan-out per session.
    outputs: HashMap<Uuid, broadcast::Sender<Vec<u8>>>,
    /// New sessions land on this devenv. Set by the launcher.
    current_ident: String,
    pending_spawns: HashMap<Uuid, (String, oneshot::Sender<Result<(), String>>)>,
    pending_snapshots: HashMap<Uuid, (String, oneshot::Sender<Vec<u8>>)>,
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
                state: Mutex::new(LinkState {
                    current_ident: DEFAULT_IDENT.to_string(),
                    ..LinkState::default()
                }),
                events,
            }),
            events_rx,
        )
    }

    /// Where new sessions go. The launcher sets this to the current toolset
    /// image's identity; `None` means the default (child-mode) devenv.
    pub async fn set_current_ident(&self, ident: Option<String>) {
        self.state.lock().await.current_ident = ident.unwrap_or_else(|| DEFAULT_IDENT.to_string());
    }

    pub async fn current_ident(&self) -> String {
        self.state.lock().await.current_ident.clone()
    }

    /// Listen for devenv connections on the shared run volume.
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

        // Unknown until the hello; the connection is not routable before it.
        let mut connection_ident: Option<String> = None;

        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(msg) = decode_line::<DevenvToNode>(&line) else {
                tracing::warn!("node: undecodable line from devenv ignored");
                continue;
            };
            match msg {
                DevenvToNode::Hello {
                    pid,
                    ident,
                    sessions,
                } => {
                    let key = ident.unwrap_or_else(|| DEFAULT_IDENT.to_string());
                    let ids: Vec<Uuid> = sessions.iter().map(|entry| entry.id).collect();
                    {
                        let mut state = self.state.lock().await;
                        // A newer connection with the same ident supersedes
                        // (child respawn, container restart); its pending
                        // RPCs can never answer.
                        fail_pending_for(&mut state, &key);
                        state.connections.insert(key.clone(), outbound_tx.clone());
                        // Sessions this devenv no longer hosts are gone —
                        // dropping their fan-out closes every subscriber.
                        // Other devenvs' sessions are untouched.
                        let stale: Vec<Uuid> = state
                            .session_host
                            .iter()
                            .filter(|(id, host)| *host == &key && !ids.contains(id))
                            .map(|(id, _)| *id)
                            .collect();
                        for id in stale {
                            state.session_host.remove(&id);
                            state.outputs.remove(&id);
                        }
                        for id in &ids {
                            state.session_host.insert(*id, key.clone());
                            state
                                .outputs
                                .entry(*id)
                                .or_insert_with(|| broadcast::channel(OUTPUT_CAPACITY).0);
                        }
                    }
                    connection_ident = Some(key.clone());
                    tracing::info!(devenv_pid = pid, ident = %key, sessions = ids.len(), "devenv connected");
                    let _ = self.events.send(LinkEvent::Connected {
                        ident: key,
                        sessions: ids,
                    });
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
                    if let (Some((_, waiter)), Ok(bytes)) = (waiter, decode_bytes(&data)) {
                        let _ = waiter.send(bytes);
                    }
                }
                DevenvToNode::SpawnResult {
                    reply_id,
                    ok,
                    error,
                } => {
                    let waiter = self.state.lock().await.pending_spawns.remove(&reply_id);
                    if let Some((_, waiter)) = waiter {
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
                    {
                        let mut state = self.state.lock().await;
                        state.outputs.remove(&id);
                        state.session_host.remove(&id);
                    }
                    let _ = self.events.send(LinkEvent::Exited { id, exit_code });
                }
                DevenvToNode::Unknown => {
                    tracing::debug!("node: unknown devenv message kind ignored");
                }
            }
        }

        if let Some(key) = connection_ident {
            let mut state = self.state.lock().await;
            // Only clear the connection if a newer one has not replaced it.
            if state
                .connections
                .get(&key)
                .is_some_and(|tx| tx.same_channel(&outbound_tx))
            {
                state.connections.remove(&key);
            }
            fail_pending_for(&mut state, &key);
        }
        drop(outbound_tx);
        let _ = writer.await;
    }

    pub async fn connected(&self) -> bool {
        !self.state.lock().await.connections.is_empty()
    }

    /// Ids of sessions any connected devenv hosts, from this link's view.
    pub async fn live_ids(&self) -> Vec<Uuid> {
        let mut ids: Vec<Uuid> = self.state.lock().await.outputs.keys().copied().collect();
        ids.sort();
        ids
    }

    /// The node-side fan-out for a session's output.
    pub async fn output_sender(&self, id: Uuid) -> Option<broadcast::Sender<Vec<u8>>> {
        self.state.lock().await.outputs.get(&id).cloned()
    }

    /// Spawns on the current devenv and returns the ident it landed on.
    pub async fn spawn(&self, spec: HostSpawnSpec) -> anyhow::Result<String> {
        let id = spec.id;
        let reply_id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        let (line, target) = {
            let mut state = self.state.lock().await;
            let target = state.current_ident.clone();
            if !state.connections.contains_key(&target) {
                anyhow::bail!("devenv {target} is not connected");
            }
            // Register the fan-out and host before the wire message so no
            // output frame can arrive ahead of them.
            state
                .outputs
                .entry(id)
                .or_insert_with(|| broadcast::channel(OUTPUT_CAPACITY).0);
            state.session_host.insert(id, target.clone());
            state.pending_spawns.insert(reply_id, (target.clone(), tx));
            (
                encode_line(&NodeToDevenv::Spawn { reply_id, spec })?,
                target,
            )
        };
        if let Err(err) = self.send_to(&target, line).await {
            let mut state = self.state.lock().await;
            state.pending_spawns.remove(&reply_id);
            state.outputs.remove(&id);
            state.session_host.remove(&id);
            return Err(err);
        }
        let result = tokio::time::timeout(SPAWN_TIMEOUT, rx).await;
        match result {
            Ok(Ok(Ok(()))) => Ok(target),
            Ok(Ok(Err(message))) => {
                let mut state = self.state.lock().await;
                state.outputs.remove(&id);
                state.session_host.remove(&id);
                Err(anyhow::anyhow!("{message}"))
            }
            Ok(Err(_)) | Err(_) => {
                let mut state = self.state.lock().await;
                state.pending_spawns.remove(&reply_id);
                state.outputs.remove(&id);
                state.session_host.remove(&id);
                Err(anyhow::anyhow!("devenv did not answer the spawn"))
            }
        }
    }

    pub async fn input(&self, id: Uuid, bytes: Vec<u8>) -> anyhow::Result<()> {
        self.send_for_session(
            id,
            encode_line(&NodeToDevenv::Input {
                id,
                data: encode_bytes(&bytes),
            })?,
        )
        .await
    }

    pub async fn resize(&self, id: Uuid, rows: u16, cols: u16) -> anyhow::Result<()> {
        self.send_for_session(id, encode_line(&NodeToDevenv::Resize { id, rows, cols })?)
            .await
    }

    pub async fn kill(&self, id: Uuid) -> anyhow::Result<()> {
        self.send_for_session(id, encode_line(&NodeToDevenv::Kill { id })?)
            .await
    }

    pub async fn snapshot(&self, id: Uuid) -> anyhow::Result<Vec<u8>> {
        let reply_id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        let (line, host) = {
            let mut state = self.state.lock().await;
            let host = state
                .session_host
                .get(&id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("session {id} has no connected devenv"))?;
            state.pending_snapshots.insert(reply_id, (host.clone(), tx));
            (
                encode_line(&NodeToDevenv::SnapshotRequest { id, reply_id })?,
                host,
            )
        };
        if let Err(err) = self.send_to(&host, line).await {
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

    async fn send_for_session(&self, id: Uuid, line: String) -> anyhow::Result<()> {
        let host = self
            .state
            .lock()
            .await
            .session_host
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("session {id} has no connected devenv"))?;
        self.send_to(&host, line).await
    }

    /// Enqueue while holding the state lock: node requests run concurrently,
    /// and two Input frames released between "fetch the sender" and
    /// "enqueue" can swap — observed as transposed keystrokes in the shell.
    /// The writer task drains the queue without this lock, so holding it
    /// across the send cannot deadlock.
    async fn send_to(&self, ident: &str, line: String) -> anyhow::Result<()> {
        let state = self.state.lock().await;
        let outbound = state
            .connections
            .get(ident)
            .ok_or_else(|| anyhow::anyhow!("devenv {ident} is not connected"))?;
        outbound
            .send(line)
            .await
            .map_err(|_| anyhow::anyhow!("devenv connection closed"))
    }
}

/// Fails the pending RPCs that were waiting on one devenv's connection,
/// leaving other devenvs' RPCs in flight.
fn fail_pending_for(state: &mut LinkState, ident: &str) {
    let spawn_ids: Vec<Uuid> = state
        .pending_spawns
        .iter()
        .filter(|(_, (host, _))| host == ident)
        .map(|(id, _)| *id)
        .collect();
    for id in spawn_ids {
        if let Some((_, waiter)) = state.pending_spawns.remove(&id) {
            let _ = waiter.send(Err("devenv connection lost".into()));
        }
    }
    let snapshot_ids: Vec<Uuid> = state
        .pending_snapshots
        .iter()
        .filter(|(_, (host, _))| host == ident)
        .map(|(id, _)| *id)
        .collect();
    for id in snapshot_ids {
        state.pending_snapshots.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devenv::server::DevenvServer;

    async fn connect_server(link: &Arc<DevenvLink>, ident: Option<&str>) -> Arc<DevenvServer> {
        let server = Arc::new(DevenvServer::with_ident(ident.map(String::from)));
        let (node_side, devenv_side) = tokio::io::duplex(1024 * 1024);
        tokio::spawn(server.clone().serve(devenv_side));
        tokio::spawn(link.clone().handle_connection(node_side));
        server
    }

    /// Waits until every named devenv has announced itself, returning the
    /// last inventory per ident. Connections race, so a single-ident wait
    /// would eat the other devenv's event.
    async fn wait_connected_all(
        events: &mut mpsc::UnboundedReceiver<LinkEvent>,
        idents: &[&str],
    ) -> HashMap<String, Vec<Uuid>> {
        let mut seen: HashMap<String, Vec<Uuid>> = HashMap::new();
        while idents.iter().any(|ident| !seen.contains_key(*ident)) {
            match tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .expect("event before deadline")
                .expect("events open")
            {
                LinkEvent::Connected { ident, sessions } => {
                    seen.insert(ident, sessions);
                }
                LinkEvent::Exited { .. } => continue,
            }
        }
        seen
    }

    async fn wait_connected(
        events: &mut mpsc::UnboundedReceiver<LinkEvent>,
        want_ident: &str,
    ) -> Vec<Uuid> {
        wait_connected_all(events, &[want_ident])
            .await
            .remove(want_ident)
            .expect("awaited ident present")
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

    async fn read_marker(output: &mut broadcast::Receiver<Vec<u8>>, needle: &str) -> String {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while !String::from_utf8_lossy(&seen).contains(needle) {
            let chunk = tokio::time::timeout_at(deadline, output.recv())
                .await
                .expect("output before deadline");
            match chunk {
                Ok(bytes) => seen.extend_from_slice(&bytes),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        String::from_utf8_lossy(&seen).into_owned()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn routes_sessions_to_their_hosting_devenv() {
        let (link, mut events) = DevenvLink::new();
        let _old = connect_server(&link, Some("old-tag")).await;
        let _new = connect_server(&link, Some("new-tag")).await;
        wait_connected_all(&mut events, &["old-tag", "new-tag"]).await;

        // A session spawned while "old-tag" was current...
        link.set_current_ident(Some("old-tag".into())).await;
        let old_spec = spec("sleep 1; echo on-old; sleep 30");
        let old_id = old_spec.id;
        assert_eq!(link.spawn(old_spec).await.expect("spawn old"), "old-tag");

        // ...keeps being served after the current tag rolls forward.
        link.set_current_ident(Some("new-tag".into())).await;
        let new_spec = spec("sleep 1; echo on-new; sleep 30");
        let new_id = new_spec.id;
        assert_eq!(link.spawn(new_spec).await.expect("spawn new"), "new-tag");

        let mut old_output = link
            .output_sender(old_id)
            .await
            .expect("old fan-out")
            .subscribe();
        let mut new_output = link
            .output_sender(new_id)
            .await
            .expect("new fan-out")
            .subscribe();
        assert!(read_marker(&mut old_output, "on-old")
            .await
            .contains("on-old"));
        assert!(read_marker(&mut new_output, "on-new")
            .await
            .contains("on-new"));

        // Snapshots route to each session's own host.
        let old_snapshot = link.snapshot(old_id).await.expect("old snapshot");
        assert!(String::from_utf8_lossy(&old_snapshot).contains("on-old"));
        let new_snapshot = link.snapshot(new_id).await.expect("new snapshot");
        assert!(String::from_utf8_lossy(&new_snapshot).contains("on-new"));

        // Await both exits: an un-reaped session still holds its PTY master,
        // and a blocked reader keeps the test runtime from shutting down.
        link.kill(old_id).await.expect("kill old");
        link.kill(new_id).await.expect("kill new");
        let mut remaining = vec![old_id, new_id];
        while !remaining.is_empty() {
            match tokio::time::timeout(Duration::from_secs(10), events.recv())
                .await
                .expect("exit before deadline")
                .expect("events open")
            {
                LinkEvent::Exited { id, .. } => remaining.retain(|r| *r != id),
                LinkEvent::Connected { .. } => continue,
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn spawn_output_snapshot_and_exit_round_trip() {
        let (link, mut events) = DevenvLink::new();
        let _server = connect_server(&link, None).await;
        assert!(wait_connected(&mut events, DEFAULT_IDENT).await.is_empty());
        assert!(link.connected().await);

        // The pause keeps the echo from racing this test's subscription: the
        // node-side fan-out drops frames that arrive with no subscriber, by
        // design — an attach recovers them from the snapshot instead.
        let spec = spec("sleep 1; echo link-marker; sleep 30");
        let id = spec.id;
        let mut output = {
            assert_eq!(link.spawn(spec).await.expect("spawn"), DEFAULT_IDENT);
            link.output_sender(id).await.expect("fan-out").subscribe()
        };

        assert!(read_marker(&mut output, "link-marker")
            .await
            .contains("link-marker"));

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
