//! The link between `wdc mcp` and the open app (plan, section 4.6).
//!
//! `wdc mcp` connects to the app through a local named pipe that only the same Windows user
//! can open; every agent call then goes through the app, which shows it in its activity
//! panel. The human can pause the agent (its calls wait) or disconnect it (it gets an error,
//! and new agent connections are refused until the human allows them again).
//!
//! Wire format: one JSON object per line, in both directions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[cfg(windows)]
pub mod pipe;

/// Message from `wdc mcp` to the app.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello { client: String, client_version: String },
    Call { id: u64, tool: String, args: serde_json::Value },
    Bye,
}

/// Message from the app to `wdc mcp`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome { app_version: String },
    Refused { reason: String },
    Result { id: u64, ok: bool, result: serde_json::Value, error: Option<String> },
    Disconnected { reason: String },
}

/// Reason given to an agent the human disconnected.
pub const DISCONNECTED_BY_HUMAN: &str = "disconnected by the user in the WinDevCleaner app";
/// Reason given when the human has refused new agent connections.
pub const REFUSED_BY_HUMAN: &str =
    "agent connections are turned off in the WinDevCleaner app (the user disconnected an agent)";
/// Reason given when another agent is already connected.
pub const REFUSED_BUSY: &str = "another agent is connected to the WinDevCleaner app";

/// A line-oriented, bidirectional connection.
pub trait LineConn {
    /// Waits up to `timeout` for one line. `Ok(None)` on timeout; an error when the peer
    /// is gone.
    fn poll_line(&mut self, timeout: Duration) -> std::io::Result<Option<String>>;
    fn send_line(&mut self, line: &str) -> std::io::Result<()>;
}

impl<T: LineConn + ?Sized> LineConn for Box<T> {
    fn poll_line(&mut self, timeout: Duration) -> std::io::Result<Option<String>> {
        (**self).poll_line(timeout)
    }
    fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        (**self).send_line(line)
    }
}

/// What the human sees and controls about agents. Shared by the app's window and the link.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkState {
    /// Name of the connected agent, as its MCP client reports it.
    pub client: Option<String>,
    pub paused: bool,
    /// The human disconnected an agent; new connections are refused until allowed again.
    pub refusing: bool,
}

#[derive(Default)]
struct Inner {
    state: LinkState,
    disconnect: bool,
    session: u64,
}

/// Pause / disconnect controls, driven by the app's buttons.
#[derive(Default)]
pub struct LinkControl {
    inner: Mutex<Inner>,
    cv: Condvar,
    cancel: Arc<AtomicBool>,
}

impl LinkControl {
    pub fn new() -> Arc<LinkControl> {
        Arc::new(LinkControl::default())
    }

    pub fn state(&self) -> LinkState {
        self.inner.lock().unwrap().state.clone()
    }

    /// "Pause agent": the agent's next calls wait until resumed.
    pub fn pause(&self) {
        self.inner.lock().unwrap().state.paused = true;
        self.cv.notify_all();
    }

    /// "Resume agent".
    pub fn resume(&self) {
        self.inner.lock().unwrap().state.paused = false;
        self.cv.notify_all();
    }

    /// "Disconnect agent": the connected agent gets an error, a running call is cancelled,
    /// and new agent connections are refused until [`LinkControl::allow`].
    pub fn disconnect(&self) {
        let mut i = self.inner.lock().unwrap();
        i.state.refusing = true;
        i.state.paused = false;
        if i.state.client.is_some() {
            i.disconnect = true;
            self.cancel.store(true, Ordering::SeqCst);
        }
        drop(i);
        self.cv.notify_all();
    }

    /// "Allow agent connections" after a disconnect.
    pub fn allow(&self) {
        self.inner.lock().unwrap().state.refusing = false;
        self.cv.notify_all();
    }

    /// Cancellation flag for the running call; set when the human disconnects the agent.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    fn attach(&self, client: &str) -> Result<u64, &'static str> {
        let mut i = self.inner.lock().unwrap();
        if i.state.refusing {
            return Err(REFUSED_BY_HUMAN);
        }
        if i.state.client.is_some() {
            return Err(REFUSED_BUSY);
        }
        i.state.client = Some(client.to_string());
        i.state.paused = false;
        i.disconnect = false;
        i.session += 1;
        self.cancel.store(false, Ordering::SeqCst);
        Ok(i.session)
    }

    fn detach(&self, session: u64) {
        let mut i = self.inner.lock().unwrap();
        if i.session == session {
            i.state.client = None;
            i.state.paused = false;
            i.disconnect = false;
        }
        drop(i);
        self.cv.notify_all();
    }

    fn disconnect_requested(&self) -> bool {
        self.inner.lock().unwrap().disconnect
    }

    fn paused(&self) -> bool {
        self.inner.lock().unwrap().state.paused
    }

    /// Blocks while the agent is paused. Returns false if the human disconnected meanwhile.
    fn wait_while_paused(&self) -> bool {
        let mut i = self.inner.lock().unwrap();
        while i.state.paused && !i.disconnect {
            i = self.cv.wait_timeout(i, Duration::from_millis(500)).unwrap().0;
        }
        !i.disconnect
    }
}

/// The app's side of a session: shows the agent's activity and runs its calls.
pub trait AgentHandler: Send + Sync {
    fn connected(&self, client: &str);
    fn disconnected(&self, client: &str, reason: &str);
    /// A call arrived; returns the activity entry id.
    fn call_started(&self, client: &str, tool: &str, args: &serde_json::Value, waiting: bool) -> u64;
    /// A call that was waiting for a paused agent now runs.
    fn call_resumed(&self, entry: u64);
    /// Runs the call. `cancel` is set if the human disconnects the agent meanwhile.
    fn call(&self, tool: &str, args: &serde_json::Value, cancel: &AtomicBool) -> Result<serde_json::Value, String>;
    fn call_finished(&self, entry: u64, result: &Result<serde_json::Value, String>);
}

fn send(conn: &mut dyn LineConn, msg: &ServerMsg) -> std::io::Result<()> {
    conn.send_line(&serde_json::to_string(msg).expect("message serializes"))
}

/// Serves one connected agent until it leaves or the human disconnects it.
pub fn serve_session(conn: &mut dyn LineConn, control: &LinkControl, handler: &dyn AgentHandler) {
    // The first line must be a hello, within a few seconds.
    let deadline = Instant::now() + Duration::from_secs(10);
    let client = loop {
        if Instant::now() > deadline {
            return;
        }
        match conn.poll_line(Duration::from_millis(200)) {
            Ok(Some(line)) => match serde_json::from_str::<ClientMsg>(&line) {
                Ok(ClientMsg::Hello { client, .. }) => {
                    let name: String = client.chars().filter(|c| !c.is_control()).take(80).collect();
                    break if name.trim().is_empty() { "unnamed MCP client".to_string() } else { name };
                }
                _ => return,
            },
            Ok(None) => continue,
            Err(_) => return,
        }
    };
    let session = match control.attach(&client) {
        Ok(s) => s,
        Err(reason) => {
            let _ = send(conn, &ServerMsg::Refused { reason: reason.to_string() });
            return;
        }
    };
    if send(conn, &ServerMsg::Welcome { app_version: crate::VERSION.to_string() }).is_err() {
        control.detach(session);
        return;
    }
    handler.connected(&client);

    let reason = loop {
        if control.disconnect_requested() {
            let _ = send(conn, &ServerMsg::Disconnected { reason: DISCONNECTED_BY_HUMAN.to_string() });
            break DISCONNECTED_BY_HUMAN.to_string();
        }
        let line = match conn.poll_line(Duration::from_millis(100)) {
            Ok(Some(l)) => l,
            Ok(None) => continue,
            Err(_) => break "the agent closed the connection".to_string(),
        };
        match serde_json::from_str::<ClientMsg>(&line) {
            Ok(ClientMsg::Call { id, tool, args }) => {
                let paused = control.paused();
                let entry = handler.call_started(&client, &tool, &args, paused);
                // A paused agent's calls wait here until the human resumes or disconnects.
                // GUARD-BEGIN(pause-gate)
                if paused {
                    if !control.wait_while_paused() {
                        let err = Err(DISCONNECTED_BY_HUMAN.to_string());
                        handler.call_finished(entry, &err);
                        let _ = send(
                            conn,
                            &ServerMsg::Result { id, ok: false, result: serde_json::Value::Null, error: Some(DISCONNECTED_BY_HUMAN.into()) },
                        );
                        continue;
                    }
                    handler.call_resumed(entry);
                }
                // GUARD-END(pause-gate)
                let cancel = control.cancel_flag();
                let mut result = handler.call(&tool, &args, &cancel);
                if control.disconnect_requested() {
                    result = Err(DISCONNECTED_BY_HUMAN.to_string());
                }
                handler.call_finished(entry, &result);
                let msg = match result {
                    Ok(v) => ServerMsg::Result { id, ok: true, result: v, error: None },
                    Err(e) => ServerMsg::Result { id, ok: false, result: serde_json::Value::Null, error: Some(e) },
                };
                if send(conn, &msg).is_err() {
                    break "the agent closed the connection".to_string();
                }
            }
            Ok(ClientMsg::Bye) => break "the agent ended the session".to_string(),
            Ok(ClientMsg::Hello { .. }) | Err(_) => {
                break "the agent sent an invalid message".to_string();
            }
        }
    };
    handler.disconnected(&client, &reason);
    control.detach(session);
}

/// The `wdc mcp` side: sends calls to the app and waits for results.
pub struct Client<C: LineConn> {
    conn: C,
    next_id: u64,
    gone: Option<String>,
}

/// How a call through the app ended.
#[derive(Debug, PartialEq)]
pub enum CallOutcome {
    Ok(serde_json::Value),
    Failed(String),
    /// The app disconnected this agent or went away; every later call fails the same way.
    Gone(String),
}

impl<C: LineConn> Client<C> {
    /// Says hello. `Err` carries the app's refusal reason or the I/O error.
    pub fn handshake(mut conn: C, client: &str) -> Result<Client<C>, String> {
        let hello = ClientMsg::Hello { client: client.to_string(), client_version: crate::VERSION.to_string() };
        conn.send_line(&serde_json::to_string(&hello).unwrap()).map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if Instant::now() > deadline {
                return Err("the WinDevCleaner app did not answer".into());
            }
            match conn.poll_line(Duration::from_millis(200)) {
                Ok(Some(line)) => {
                    return match serde_json::from_str::<ServerMsg>(&line) {
                        Ok(ServerMsg::Welcome { .. }) => Ok(Client { conn, next_id: 1, gone: None }),
                        Ok(ServerMsg::Refused { reason }) => Err(reason),
                        _ => Err("unexpected answer from the WinDevCleaner app".into()),
                    }
                }
                Ok(None) => continue,
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// Sends a call and waits for its result (a paused agent waits here).
    pub fn call(&mut self, tool: &str, args: serde_json::Value) -> CallOutcome {
        if let Some(r) = &self.gone {
            return CallOutcome::Gone(r.clone());
        }
        let id = self.next_id;
        self.next_id += 1;
        let msg = ClientMsg::Call { id, tool: tool.to_string(), args };
        if let Err(e) = self.conn.send_line(&serde_json::to_string(&msg).unwrap()) {
            return self.gone(format!("the WinDevCleaner app closed the connection ({e})"));
        }
        loop {
            match self.conn.poll_line(Duration::from_millis(500)) {
                Ok(Some(line)) => match serde_json::from_str::<ServerMsg>(&line) {
                    Ok(ServerMsg::Result { id: rid, ok, result, error }) if rid == id => {
                        if !ok && error.as_deref() == Some(DISCONNECTED_BY_HUMAN) {
                            return self.gone(DISCONNECTED_BY_HUMAN.to_string());
                        }
                        return if ok {
                            CallOutcome::Ok(result)
                        } else {
                            CallOutcome::Failed(error.unwrap_or_else(|| "failed".into()))
                        };
                    }
                    Ok(ServerMsg::Disconnected { reason }) => return self.gone(reason),
                    _ => continue,
                },
                Ok(None) => continue,
                Err(e) => return self.gone(format!("the WinDevCleaner app closed the connection ({e})")),
            }
        }
    }

    fn gone(&mut self, reason: String) -> CallOutcome {
        self.gone = Some(reason.clone());
        CallOutcome::Gone(reason)
    }

    pub fn bye(&mut self) {
        let _ = self.conn.send_line(&serde_json::to_string(&ClientMsg::Bye).unwrap());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    /// A socket pair stands in for the named pipe in these platform-independent tests; the
    /// pipe itself is tested on Windows (tests/pipe.rs).
    struct Sock {
        reader: BufReader<UnixStream>,
        writer: UnixStream,
    }
    impl LineConn for Sock {
        fn poll_line(&mut self, timeout: Duration) -> std::io::Result<Option<String>> {
            self.reader.get_ref().set_read_timeout(Some(timeout))?;
            let mut s = String::new();
            match self.reader.read_line(&mut s) {
                Ok(0) => Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(_) => Ok(Some(s.trim_end().to_string())),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut => Ok(None),
                Err(e) => Err(e),
            }
        }
        fn send_line(&mut self, line: &str) -> std::io::Result<()> {
            writeln!(self.writer, "{line}")
        }
    }
    fn pair() -> (Sock, Sock) {
        let (a, b) = UnixStream::pair().unwrap();
        (
            Sock { reader: BufReader::new(a.try_clone().unwrap()), writer: a },
            Sock { reader: BufReader::new(b.try_clone().unwrap()), writer: b },
        )
    }

    struct Echo {
        log: Mutex<Vec<String>>,
    }
    impl AgentHandler for Echo {
        fn connected(&self, client: &str) {
            self.log.lock().unwrap().push(format!("connected {client}"));
        }
        fn disconnected(&self, client: &str, reason: &str) {
            self.log.lock().unwrap().push(format!("disconnected {client}: {reason}"));
        }
        fn call_started(&self, _: &str, tool: &str, _: &serde_json::Value, waiting: bool) -> u64 {
            self.log.lock().unwrap().push(format!("start {tool} waiting={waiting}"));
            1
        }
        fn call_resumed(&self, _: u64) {
            self.log.lock().unwrap().push("resumed".into());
        }
        fn call(&self, tool: &str, args: &serde_json::Value, _: &AtomicBool) -> Result<serde_json::Value, String> {
            Ok(serde_json::json!({ "tool": tool, "args": args }))
        }
        fn call_finished(&self, _: u64, _: &Result<serde_json::Value, String>) {}
    }

    #[test]
    fn calls_round_trip_and_disconnect_refuses_new_agents() {
        let control = LinkControl::new();
        let handler = Arc::new(Echo { log: Mutex::new(vec![]) });
        let (mut server_side, client_side) = pair();
        let (c2, h2) = (control.clone(), handler.clone());
        let t = std::thread::spawn(move || serve_session(&mut server_side, &c2, h2.as_ref()));
        let mut client = Client::handshake(client_side, "Test Agent").unwrap();
        assert_eq!(control.state().client.as_deref(), Some("Test Agent"));
        match client.call("scan", serde_json::json!({"x": 1})) {
            CallOutcome::Ok(v) => assert_eq!(v["tool"], "scan"),
            other => panic!("{other:?}"),
        }
        control.disconnect();
        t.join().unwrap();
        assert!(matches!(client.call("scan", serde_json::json!({})), CallOutcome::Gone(_)));
        assert_eq!(control.state().client, None);

        // A new agent is refused until the human allows connections again.
        let (mut s2, c2side) = pair();
        let c3 = control.clone();
        let h3 = handler.clone();
        let t = std::thread::spawn(move || serve_session(&mut s2, &c3, h3.as_ref()));
        assert_eq!(Client::handshake(c2side, "Other").err().as_deref(), Some(REFUSED_BY_HUMAN));
        t.join().unwrap();
        control.allow();
        assert!(!control.state().refusing);
    }
}
