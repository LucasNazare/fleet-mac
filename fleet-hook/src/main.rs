//! fleet-hook: registered as a Claude Code command hook by fleet.
//! Reads the hook JSON from stdin, tags it with FLEET_SESSION_ID and writes one
//! line to FLEET_SOCKET. Prints nothing, never blocks for long, always exits 0,
//! so it can never change Claude Code's behaviour.
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::Duration;

const MAX_INPUT: u64 = 4 * 1024 * 1024;

fn main() {
    let _ = run();
    std::process::exit(0);
}

fn run() -> Option<()> {
    let session = std::env::var("FLEET_SESSION_ID").ok()?;
    let socket = std::env::var("FLEET_SOCKET").ok()?;

    // Read stdin on a thread so a stuck pipe can't hold us past 1 s.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::stdin().take(MAX_INPUT).read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let input = rx.recv_timeout(Duration::from_secs(1)).ok()?;
    let payload: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(serde_json::Value::String(input));

    let line = serde_json::json!({ "fleet_session_id": session, "payload": payload }).to_string();
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.write_all(line.as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;
    Some(())
}
