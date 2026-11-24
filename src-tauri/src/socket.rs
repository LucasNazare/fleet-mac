//! The event bus: a Unix socket that `fleet-hook` writes newline-delimited JSON to.
use crate::hooks::Envelope;
use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::Duration;

pub fn serve(path: &Path, on_event: impl Fn(Envelope) + Send + Sync + 'static) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    let on_event = std::sync::Arc::new(on_event);
    std::thread::Builder::new().name("fleet-socket".into()).spawn(move || {
        for conn in listener.incoming().flatten() {
            let on_event = on_event.clone();
            std::thread::spawn(move || {
                let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
                for line in BufReader::new(conn).lines().map_while(Result::ok) {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Envelope>(&line) {
                        Ok(env) => on_event(env),
                        Err(e) => eprintln!("[fleet] bad hook line: {e}"),
                    }
                }
            });
        }
    })?;
    Ok(())
}
