//! The remote wake listener (spec §12): one background thread holding the
//! server's freshness WebSocket open and nudging the engine's worker whenever
//! the journal advances, so another device's edit lands here in seconds rather
//! than on the safety timer.
//!
//! The socket is a hint channel only. Frames carry no data, and a missed or
//! dropped frame costs nothing, because cycles are cursor-driven and
//! idempotent. The listener therefore reconnects forever (with backoff) and
//! treats a long-quiet connection as a cue to rebuild it, which also heals the
//! silently dead sockets that NAT timeouts and phone sleep leave behind.

use std::io::ErrorKind;
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tungstenite::client::IntoClientRequest;
use tungstenite::{Message, WebSocket};

use crate::engine::RemoteWakeConfig;

/// How long a connection may stay silent before it is torn down and rebuilt.
/// Silence is the normal state (the server only speaks when the journal
/// advances), so this is not a health probe interval; it just bounds how long
/// a dead socket can go unnoticed, and rebuilding is cheap on the tailnet.
const QUIET_REBUILD: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Reconnect backoff after a failed connect or a dropped connection, doubling
/// up to the cap. A successful handshake resets it.
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// The running listener. Dropping it (or calling [`RemoteWake::stop`]) shuts
/// the live socket down, which unblocks the reader immediately, and joins the
/// thread.
pub(crate) struct RemoteWake {
    stop: Arc<AtomicBool>,
    /// The live stream, kept so `stop()` can shut it down out from under the
    /// blocked read instead of waiting out the read timeout.
    live: Arc<Mutex<Option<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
}

impl RemoteWake {
    /// Start listening; `wake` is called (from the listener thread) once per
    /// journal-advance frame. It must be cheap and must not block.
    pub fn spawn(config: RemoteWakeConfig, wake: impl Fn() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let live = Arc::new(Mutex::new(None));
        let handle = {
            let stop = stop.clone();
            let live = live.clone();
            std::thread::spawn(move || listen_loop(config, wake, stop, live))
        };
        Self {
            stop,
            live,
            handle: Some(handle),
        }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(stream) = self.live.lock().unwrap().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for RemoteWake {
    fn drop(&mut self) {
        self.stop();
    }
}

fn listen_loop(
    config: RemoteWakeConfig,
    wake: impl Fn(),
    stop: Arc<AtomicBool>,
    live: Arc<Mutex<Option<TcpStream>>>,
) {
    let mut backoff = BACKOFF_START;
    while !stop.load(Ordering::SeqCst) {
        match connect(&config) {
            Ok(mut ws) => {
                backoff = BACKOFF_START;
                *live.lock().unwrap() = ws.get_ref().try_clone().ok();
                // `stop()` may have raced the store above; bail rather than
                // sit in a read it can no longer interrupt.
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                read_until_dead(&mut ws, &wake, &stop);
                *live.lock().unwrap() = None;
                // A rebuild after a healthy connection retries immediately;
                // only failures back off.
            }
            Err(_) => {
                sleep_checked(backoff, &stop);
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
}

/// Read frames until the connection errors, closes, or stays quiet past
/// [`QUIET_REBUILD`]. Every journal-advance frame becomes one `wake()` call.
fn read_until_dead(ws: &mut WebSocket<TcpStream>, wake: &impl Fn(), stop: &AtomicBool) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match ws.read() {
            // The frame content doesn't matter (it is `{"seq":N}`): any frame
            // from the server means the journal moved, so run a cycle.
            Ok(Message::Text(_)) | Ok(Message::Binary(_)) => wake(),
            Ok(_) => {} // ping/pong/close-in-progress noise
            Err(tungstenite::Error::Io(e))
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut =>
            {
                return; // quiet too long: rebuild the connection
            }
            Err(_) => return, // dead or closed: rebuild
        }
    }
}

/// Dial the server and complete the WebSocket handshake on the wake route,
/// authenticating with the device token.
fn connect(config: &RemoteWakeConfig) -> anyhow::Result<WebSocket<TcpStream>> {
    let host_port = host_port(&config.server_url)?;
    let addr = host_port
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("no address for {host_port}"))?;
    let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(QUIET_REBUILD))?;

    let url = format!("ws://{host_port}/v1/vaults/{}/ws", config.vault_id);
    let mut request = url.into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.token).parse()?,
    );
    let (ws, _response) = tungstenite::client::client(request, stream)?;
    Ok(ws)
}

/// `http://host:port[/...]` → `host:port` (port defaulting to 80).
fn host_port(server_url: &str) -> anyhow::Result<String> {
    let rest = server_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("wake needs an http:// server url, got {server_url}"))?;
    let authority = rest.split('/').next().unwrap_or(rest);
    if authority.is_empty() {
        anyhow::bail!("no host in server url {server_url}");
    }
    Ok(if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    })
}

/// Sleep in short slices so `stop()` never waits out a whole backoff.
fn sleep_checked(total: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(100);
    let mut remaining = total;
    while !remaining.is_zero() && !stop.load(Ordering::SeqCst) {
        let step = remaining.min(slice);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;

    #[test]
    fn host_port_parses_origins() {
        assert_eq!(host_port("http://100.64.0.1:8787").unwrap(), "100.64.0.1:8787");
        assert_eq!(host_port("http://mini:8787/").unwrap(), "mini:8787");
        assert_eq!(host_port("http://mini").unwrap(), "mini:80");
        assert!(host_port("https://mini:8787").is_err());
    }

    /// A frame from the server calls `wake`, and the handshake carries the
    /// bearer token. The server here is a real (blocking) WebSocket accept, so
    /// this exercises the actual handshake path.
    #[test]
    fn frames_wake_and_the_handshake_authenticates() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (auth_tx, auth_rx) = mpsc::channel::<String>();

        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let callback = |req: &tungstenite::handshake::server::Request,
                            resp: tungstenite::handshake::server::Response| {
                let auth = req
                    .headers()
                    .get("Authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                auth_tx.send(auth).unwrap();
                Ok(resp)
            };
            let mut ws = tungstenite::accept_hdr(stream, callback).unwrap();
            ws.send(Message::Text("{\"seq\":7}".into())).unwrap();
            // Keep the socket open until the client is done reading.
            let _ = ws.read();
        });

        let wakes = Arc::new(AtomicU32::new(0));
        let counted = wakes.clone();
        let mut wake = RemoteWake::spawn(
            RemoteWakeConfig {
                server_url: format!("http://127.0.0.1:{port}"),
                vault_id: "default".into(),
                token: "tok-123".into(),
            },
            move || {
                counted.fetch_add(1, Ordering::SeqCst);
            },
        );

        let auth = auth_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(auth, "Bearer tok-123");
        for _ in 0..100 {
            if wakes.load(Ordering::SeqCst) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(wakes.load(Ordering::SeqCst) > 0, "no wake arrived");

        wake.stop();
        server.join().unwrap();
    }

    /// `stop()` returns promptly even while the reader is blocked on a live,
    /// silent connection (the shutdown unblocks it).
    #[test]
    fn stop_unblocks_a_silent_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let _ = ws.read(); // sit silent until the client goes away
        });

        let mut wake = RemoteWake::spawn(
            RemoteWakeConfig {
                server_url: format!("http://127.0.0.1:{port}"),
                vault_id: "default".into(),
                token: "tok".into(),
            },
            || {},
        );
        // Give the client a moment to connect and block in read.
        std::thread::sleep(Duration::from_millis(300));

        let begun = std::time::Instant::now();
        wake.stop();
        assert!(
            begun.elapsed() < Duration::from_secs(5),
            "stop took {:?}",
            begun.elapsed()
        );
        server.join().unwrap();
    }
}
