//! PTY ownership. Every claude (and later every dev process) runs in a PTY owned
//! here. Output is appended to a replay ring and to a pending buffer that a single
//! flusher thread pushes to the attached frontend channel at most every 16 ms.
use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};

/// Raw bytes kept for replaying into a freshly mounted terminal.
pub const REPLAY_BYTES: usize = 6 * 1024 * 1024;
pub const FLUSH_EVERY: Duration = Duration::from_millis(16);

pub trait Sink: Send + Sync {
    /// Returns false if the receiver is gone.
    fn send(&self, bytes: Vec<u8>) -> bool;
}

impl Sink for Channel<InvokeResponseBody> {
    fn send(&self, bytes: Vec<u8>) -> bool {
        Channel::send(self, InvokeResponseBody::Raw(bytes)).is_ok()
    }
}

#[derive(Default)]
pub struct Buffers {
    pub ring: Vec<u8>,
    pub pending: Vec<u8>,
    pub sink: Option<Arc<dyn Sink>>,
}

impl Buffers {
    pub fn push(&mut self, data: &[u8]) {
        self.ring.extend_from_slice(data);
        if self.ring.len() > REPLAY_BYTES {
            // Drop the oldest quarter, then cut at a line break so replay starts clean.
            let mut cut = self.ring.len() - REPLAY_BYTES * 3 / 4;
            if let Some(nl) = self.ring[cut..].iter().position(|b| *b == b'\n') {
                cut += nl + 1;
            }
            self.ring.drain(..cut);
        }
        if self.sink.is_some() {
            self.pending.extend_from_slice(data);
        }
    }

    /// Attach a new receiver: it gets the replay first, then live output.
    pub fn attach(&mut self, sink: Arc<dyn Sink>) {
        self.pending.clear();
        if !self.ring.is_empty() {
            sink.send(self.ring.clone());
        }
        self.sink = Some(sink);
    }

    pub fn flush(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.pending);
        if let Some(s) = &self.sink {
            if !s.send(data) {
                self.sink = None;
            }
        }
    }
}

pub struct Pty {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pub buffers: Arc<Mutex<Buffers>>,
    pub pid: Option<u32>,
    pub exited: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
pub struct PtyManager {
    ptys: RwLock<HashMap<String, Arc<Pty>>>,
}

impl PtyManager {
    pub fn new() -> Arc<Self> {
        let m = Arc::new(Self::default());
        let weak = Arc::downgrade(&m);
        std::thread::Builder::new()
            .name("pty-flusher".into())
            .spawn(move || loop {
                std::thread::sleep(FLUSH_EVERY);
                let Some(m) = weak.upgrade() else { break };
                let ptys: Vec<Arc<Pty>> = m.ptys.read().values().cloned().collect();
                for p in ptys {
                    p.buffers.lock().flush();
                }
            })
            .expect("spawn flusher");
        m
    }

    /// Spawn `cmd` in a new PTY under `id`, replacing any previous PTY with that id.
    /// `on_exit(code, current)`: `current` is false when this PTY was killed or
    /// replaced in the meantime (so the callback must not touch the new owner's state).
    /// An exited PTY stays registered (its scrollback can still be replayed) until
    /// it is replaced or killed.
    pub fn spawn(
        self: &Arc<Self>,
        id: &str,
        cmd: CommandBuilder,
        cols: u16,
        rows: u16,
        on_exit: impl FnOnce(Option<i32>, bool) + Send + 'static,
    ) -> Result<u32> {
        let pair = native_pty_system().openpty(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })?;
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let buffers = Arc::new(Mutex::new(Buffers::default()));
        let pty = Arc::new(Pty {
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            buffers: buffers.clone(),
            pid,
            exited: std::sync::atomic::AtomicBool::new(false),
        });
        if let Some(old) = self.ptys.write().insert(id.to_string(), pty.clone()) {
            if !old.exited.load(std::sync::atomic::Ordering::Relaxed) {
                Self::hangup(&old);
            }
        }
        let weak = Arc::downgrade(self);
        let id_owned = id.to_string();

        std::thread::Builder::new().name(format!("pty-read-{id}")).spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buffers.lock().push(&buf[..n]),
                }
            }
            let code = child.wait().ok().map(|s| s.exit_code() as i32);
            // Let the flusher deliver the last bytes before reporting exit.
            std::thread::sleep(FLUSH_EVERY * 2);
            buffers.lock().flush();
            pty.exited.store(true, std::sync::atomic::Ordering::Relaxed);
            let current = weak
                .upgrade()
                .map(|m| m.ptys.read().get(&id_owned).map(|p| Arc::ptr_eq(p, &pty)).unwrap_or(false))
                .unwrap_or(false);
            on_exit(code, current);
        })?;
        pid.ok_or_else(|| anyhow!("no pid"))
    }

    fn get(&self, id: &str) -> Result<Arc<Pty>> {
        self.ptys.read().get(id).cloned().ok_or_else(|| anyhow!("no such terminal: {id}"))
    }

    pub fn exists(&self, id: &str) -> bool {
        self.ptys.read().contains_key(id)
    }

    /// Running (not exited) PTY's child pid.
    pub fn live_pid(&self, id: &str) -> Option<u32> {
        let p = self.ptys.read().get(id).cloned()?;
        if p.exited.load(std::sync::atomic::Ordering::Relaxed) { None } else { p.pid }
    }

    /// Send a signal to the PTY child's whole process group.
    pub fn signal_group(&self, id: &str, sig: i32) -> bool {
        let Some(pid) = self.live_pid(id) else { return false };
        signal_group(pid, sig);
        true
    }

    pub fn clear_replay(&self, id: &str) {
        if let Ok(p) = self.get(id) {
            p.buffers.lock().ring.clear();
        }
    }

    fn hangup(p: &Arc<Pty>) {
        if let Some(pid) = p.pid {
            signal_group(pid, libc::SIGHUP);
            let p = p.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(3));
                if !p.exited.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = p.killer.lock().kill();
                }
            });
        } else {
            let _ = p.killer.lock().kill();
        }
    }

    pub fn attach(&self, id: &str, sink: Arc<dyn Sink>) -> Result<()> {
        self.get(id)?.buffers.lock().attach(sink);
        Ok(())
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<()> {
        let p = self.get(id)?;
        let mut w = p.writer.lock();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        self.get(id)?.master.lock().resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })?;
        Ok(())
    }

    /// Hang up (SIGHUP the process group, SIGKILL after 3 s) and unregister.
    pub fn kill(&self, id: &str) {
        if let Some(p) = self.ptys.write().remove(id) {
            if !p.exited.load(std::sync::atomic::Ordering::Relaxed) {
                Self::hangup(&p);
            }
        }
    }

    /// Kill every PTY whose id passes `filter`.
    pub fn kill_where(&self, filter: impl Fn(&str) -> bool) {
        let ids: Vec<String> = self.ptys.read().keys().filter(|k| filter(k)).cloned().collect();
        for id in ids {
            self.kill(&id);
        }
    }
}

pub fn signal_group(pid: u32, sig: i32) {
    unsafe {
        let pgid = libc::getpgid(pid as i32);
        if pgid > 0 {
            libc::killpg(pgid, sig);
        } else {
            libc::kill(pid as i32, sig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Collect(Mutex<Vec<Vec<u8>>>, AtomicUsize);
    impl Sink for Collect {
        fn send(&self, b: Vec<u8>) -> bool {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.lock().push(b);
            true
        }
    }

    #[test]
    fn replay_then_batched_live() {
        let mut b = Buffers::default();
        b.push(b"hello ");
        b.push(b"world\n");
        let sink = Arc::new(Collect(Mutex::new(vec![]), AtomicUsize::new(0)));
        b.attach(sink.clone());
        assert_eq!(sink.0.lock()[0], b"hello world\n");
        b.push(b"a");
        b.push(b"b");
        b.push(b"c");
        b.flush();
        b.flush();
        let got = sink.0.lock();
        assert_eq!(got.len(), 2, "three pushes batch into one send");
        assert_eq!(got[1], b"abc");
    }

    #[test]
    fn ring_is_capped_at_line_boundary() {
        let mut b = Buffers::default();
        let line = vec![b'x'; 1023].into_iter().chain([b'\n']).collect::<Vec<u8>>();
        for _ in 0..(REPLAY_BYTES / 1024 + 10) {
            b.push(&line);
        }
        assert!(b.ring.len() <= REPLAY_BYTES);
        assert_eq!(b.ring.len() % 1024, 0);
        assert!(b.pending.is_empty(), "no pending without a sink");
    }

    #[test]
    fn real_pty_roundtrip() {
        let m = PtyManager::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut cmd = CommandBuilder::new("/bin/sh");
        cmd.args(["-c", "read x; echo got:$x; exit 3"]);
        m.spawn("t", cmd, 80, 24, move |code, current| { let _ = tx.send((code, current)); }).unwrap();
        let sink = Arc::new(Collect(Mutex::new(vec![]), AtomicUsize::new(0)));
        m.attach("t", sink.clone()).unwrap();
        m.write("t", b"ping\n").unwrap();
        let (code, current) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(code, Some(3));
        assert!(current);
        assert!(m.exists("t"), "exited PTY keeps its replay");
        let all: Vec<u8> = sink.0.lock().concat();
        assert!(String::from_utf8_lossy(&all).contains("got:ping"));
    }

    #[test]
    fn replaced_pty_exit_is_not_current() {
        let m = PtyManager::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut a = CommandBuilder::new("/bin/sh");
        a.args(["-c", "sleep 30"]);
        let tx1 = tx.clone();
        m.spawn("x", a, 80, 24, move |_, current| { let _ = tx1.send(("a", current)); }).unwrap();
        let mut b = CommandBuilder::new("/bin/sh");
        b.args(["-c", "sleep 0.3"]);
        m.spawn("x", b, 80, 24, move |_, current| { let _ = tx.send(("b", current)); }).unwrap();
        let mut got = vec![rx.recv_timeout(Duration::from_secs(5)).unwrap(), rx.recv_timeout(Duration::from_secs(5)).unwrap()];
        got.sort();
        assert_eq!(got, vec![("a", false), ("b", true)]);
    }

    /// A server two levels below the PTY child (sh → python) is found by port
    /// detection and its port is free again after a group SIGINT.
    #[test]
    fn port_detected_and_freed_on_stop() {
        let port = 18000 + (std::process::id() % 1000) as u16;
        let m = PtyManager::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut c = CommandBuilder::new("/bin/sh");
        c.args(["-c", &format!("trap '' HUP; python3 -m http.server {port} --bind 127.0.0.1; echo done")]);
        let pid = m.spawn("srv", c, 80, 24, move |code, _| { let _ = tx.send(code); }).unwrap();
        let mut found = vec![];
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(150));
            found = crate::processes::listening_ports(&[pid]).remove(&pid).unwrap_or_default();
            if !found.is_empty() {
                break;
            }
        }
        assert_eq!(found, vec![port]);
        assert!(m.signal_group("srv", libc::SIGINT));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "port still busy");
    }

    #[test]
    fn group_signal_stops_children() {
        let m = PtyManager::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut c = CommandBuilder::new("/bin/sh");
        c.args(["-c", "sleep 30 & sleep 30; wait"]);
        m.spawn("g", c, 80, 24, move |code, _| { let _ = tx.send(code); }).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(m.signal_group("g", libc::SIGTERM));
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        assert!(m.live_pid("g").is_none());
    }
}
