//! The app link's named pipe (plan, sections 4.6 and 6.5). Windows only.
#![cfg(windows)]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use wdc_core::link::pipe::{accept, connect, create_instance, Connect};
use wdc_core::link::{serve_session, AgentHandler, CallOutcome, Client, LinkControl, DISCONNECTED_BY_HUMAN};
use wdc_testkit::{guard_assert, guard_assert_eq};

static N: AtomicU32 = AtomicU32::new(0);

fn unique_name() -> String {
    format!(r"\\.\pipe\wdc-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
}

impl AgentHandler for Recorder {
    fn connected(&self, client: &str) {
        self.events.lock().unwrap().push(format!("connected:{client}"));
    }
    fn disconnected(&self, client: &str, reason: &str) {
        self.events.lock().unwrap().push(format!("disconnected:{client}:{reason}"));
    }
    fn call_started(&self, _: &str, tool: &str, _: &serde_json::Value, waiting: bool) -> u64 {
        self.events.lock().unwrap().push(format!("started:{tool}:waiting={waiting}"));
        7
    }
    fn call_resumed(&self, _: u64) {
        self.events.lock().unwrap().push("resumed".into());
    }
    fn call(&self, tool: &str, _: &serde_json::Value, _: &AtomicBool) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({ "tool": tool }))
    }
    fn call_finished(&self, _: u64, r: &Result<serde_json::Value, String>) {
        self.events.lock().unwrap().push(format!("finished:ok={}", r.is_ok()));
    }
}

/// Starts a one-session server on a fresh pipe name.
fn server(control: Arc<LinkControl>, handler: Arc<Recorder>) -> (String, std::thread::JoinHandle<()>) {
    let name = unique_name();
    let instance = create_instance(&name, true).expect("create pipe");
    let t = std::thread::spawn(move || {
        let mut conn = accept(instance).expect("accept");
        serve_session(&mut conn, &control, handler.as_ref());
    });
    (name, t)
}

fn client(name: &str, who: &str) -> Client<wdc_core::link::pipe::PipeConn> {
    match connect(name).expect("connect") {
        Connect::Connected(c) => Client::handshake(c, who).expect("handshake"),
        Connect::NotRunning => panic!("pipe not found"),
    }
}

#[test]
fn same_user_connects_and_calls_go_through() {
    let control = LinkControl::new();
    let handler = Arc::new(Recorder::default());
    let (name, t) = server(control.clone(), handler.clone());
    let mut c = client(&name, "Claude Code");
    assert_eq!(control.state().client.as_deref(), Some("Claude Code"));
    assert_eq!(c.call("scan", serde_json::json!({})), CallOutcome::Ok(serde_json::json!({ "tool": "scan" })));
    c.bye();
    t.join().unwrap();
    let ev = handler.events.lock().unwrap().clone();
    assert_eq!(ev[0], "connected:Claude Code");
    assert!(ev.contains(&"started:scan:waiting=false".to_string()));
}

#[test]
fn a_paused_agents_calls_wait_until_resumed() {
    let control = LinkControl::new();
    let handler = Arc::new(Recorder::default());
    let (name, t) = server(control.clone(), handler.clone());
    let mut c = client(&name, "Codex");
    control.pause();

    let (tx, rx) = mpsc::channel();
    let caller = std::thread::spawn(move || {
        let started = Instant::now();
        let out = c.call("scan", serde_json::json!({}));
        tx.send((out, started.elapsed())).unwrap();
        c
    });
    // While paused, the call does not complete.
    let early = rx.recv_timeout(Duration::from_millis(1500));
    guard_assert!("pause-gate", early.is_err(), "a paused agent's call completed: {:?}", early);
    control.resume();
    let (out, waited) = rx.recv_timeout(Duration::from_secs(10)).expect("call completes after resume");
    assert_eq!(out, CallOutcome::Ok(serde_json::json!({ "tool": "scan" })));
    assert!(waited >= Duration::from_millis(1500));
    let mut c = caller.join().unwrap();
    c.bye();
    t.join().unwrap();
    let ev = handler.events.lock().unwrap().clone();
    assert!(ev.contains(&"started:scan:waiting=true".to_string()), "{ev:?}");
    assert!(ev.contains(&"resumed".to_string()), "{ev:?}");
}

#[test]
fn a_disconnected_agent_gets_an_error_and_new_agents_are_refused() {
    let control = LinkControl::new();
    let handler = Arc::new(Recorder::default());
    let (name, t) = server(control.clone(), handler.clone());
    let mut c = client(&name, "Codex");
    control.pause();
    let (tx, rx) = mpsc::channel();
    let caller = std::thread::spawn(move || {
        let out = c.call("scan", serde_json::json!({}));
        tx.send(out).unwrap();
        c
    });
    std::thread::sleep(Duration::from_millis(300));
    control.disconnect();
    let out = rx.recv_timeout(Duration::from_secs(10)).expect("waiting call ends on disconnect");
    assert_eq!(out, CallOutcome::Gone(DISCONNECTED_BY_HUMAN.to_string()));
    let mut c = caller.join().unwrap();
    // Every later call fails the same way.
    assert!(matches!(c.call("scan", serde_json::json!({})), CallOutcome::Gone(_)));
    t.join().unwrap();

    // A new agent is refused while the human has not allowed connections again.
    let (name2, t2) = server(control.clone(), handler.clone());
    let refused = match connect(&name2).unwrap() {
        Connect::Connected(conn) => Client::handshake(conn, "Another").err(),
        Connect::NotRunning => panic!("pipe not found"),
    };
    assert!(refused.unwrap().contains("turned off"));
    t2.join().unwrap();
}

#[test]
fn a_missing_pipe_means_the_app_is_closed() {
    assert!(matches!(connect(&unique_name()).unwrap(), Connect::NotRunning));
}

/// Needs a second local user, created by the CI workflow (WDC_OTHER_USER and
/// WDC_OTHER_PASSWORD). Run with `--ignored`.
#[test]
#[ignore]
fn another_user_cannot_open_the_pipe() {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    let user = std::env::var("WDC_OTHER_USER").expect("WDC_OTHER_USER");
    let password = std::env::var("WDC_OTHER_PASSWORD").expect("WDC_OTHER_PASSWORD");
    let name = unique_name();
    let _instance = create_instance(&name, true).expect("create pipe");
    let _second = create_instance(&name, false).expect("second instance");

    // Even read access only: the default access list of a pipe would grant read to Everyone.
    let as_other = wdc_testkit::as_other_user(&user, &password, || wdc_testkit::try_open(&name, GENERIC_READ));
    guard_assert_eq!("pipe-dacl", as_other, Err(5), "another user must get ERROR_ACCESS_DENIED");
    let as_other_rw =
        wdc_testkit::as_other_user(&user, &password, || wdc_testkit::try_open(&name, GENERIC_READ | GENERIC_WRITE));
    assert_eq!(as_other_rw, Err(5));
    // Positive control: the same user opens it.
    assert_eq!(wdc_testkit::try_open(&name, GENERIC_READ | GENERIC_WRITE), Ok(()));
}
