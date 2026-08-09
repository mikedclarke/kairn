//! The concrete HTTP transport (spec §10): a synchronous [`Transport`] over the
//! sync server's HTTP API. Deliberately blocking and runtime-free (`ureq`, no
//! tokio) so it drops straight into the engine's synchronous cycle and
//! cross-compiles into the iOS framework without dragging in an async runtime.
//!
//! v1 is plain HTTP over the tailnet (spec §3); the `ureq` dependency is built
//! without TLS on purpose, so `http://` is the only scheme until the server
//! moves to a VPS and this gains a `tls` feature. All request paths are
//! `/v1/vaults/{vault}/...` and every request carries the device bearer token
//! issued at enrollment (spec §10).

use std::io::Read;
use std::time::Duration;

use crate::transport::{Transport, TransportError, TransportResult};
use crate::types::{ChangesPage, ContentHash, PutOutcome, Rev, Seq, VaultPath};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// A hard ceiling on the whole call (connect + send + receive). The per-phase
/// timeouts above don't cover every stall — a connection pooled from a since-
/// restarted server, or a peer that accepts then goes silent mid-body, can hang
/// a blocking client indefinitely. This caps it so a wedged call errors and the
/// next cycle retries on a fresh connection, which matters most for the always-
/// on bridge. Generous enough for a 25 MB blob (spec §4) on a slow link.
const OVERALL_TIMEOUT: Duration = Duration::from_secs(120);

/// An HTTP client bound to one server, one vault, and one device token. Cheap to
/// construct; the underlying `ureq::Agent` pools connections across cycles.
pub struct HttpTransport {
    agent: ureq::Agent,
    /// The server origin with no trailing slash, e.g. `http://100.121.119.52:8787`.
    base: String,
    vault: String,
    token: String,
}

impl HttpTransport {
    /// Bind a transport to `server_url` (scheme + host + port), one `vault` id,
    /// and the device `token`. A trailing slash on the URL is tolerated.
    pub fn new(
        server_url: impl Into<String>,
        vault: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout_read(READ_TIMEOUT)
            .timeout(OVERALL_TIMEOUT)
            .build();
        let mut base = server_url.into();
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            agent,
            base,
            vault: vault.into(),
            token: token.into(),
        }
    }

    fn vault_base(&self) -> String {
        format!("{}/v1/vaults/{}", self.base, self.vault)
    }

    fn file_url(&self, path: &VaultPath) -> String {
        format!("{}/files/{}", self.vault_base(), encode_path(&path.0))
    }

    /// Attach the bearer token every authed request carries (spec §10).
    fn authed(&self, req: ureq::Request) -> ureq::Request {
        req.set("Authorization", &format!("Bearer {}", self.token))
    }
}

impl Transport for HttpTransport {
    fn changes(&self, since: Seq, limit: u32) -> TransportResult<ChangesPage> {
        let url = format!("{}/changes?since={since}&limit={limit}", self.vault_base());
        let resp = self.authed(self.agent.get(&url)).call().map_err(map_err)?;
        read_json(resp)
    }

    fn get_blob(&self, path: &VaultPath, rev: Option<Rev>) -> TransportResult<Vec<u8>> {
        let mut url = self.file_url(path);
        if let Some(rev) = rev {
            url.push_str(&format!("?rev={rev}"));
        }
        let resp = self.authed(self.agent.get(&url)).call().map_err(map_err)?;
        let mut buf = Vec::new();
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| TransportError::Network(e.to_string()))?;
        Ok(buf)
    }

    fn put_blob(
        &self,
        path: &VaultPath,
        base_rev: Rev,
        hash: &ContentHash,
        content: &[u8],
    ) -> TransportResult<PutOutcome> {
        let url = self.file_url(path);
        let resp = self
            .authed(self.agent.put(&url))
            .set("base-rev", &base_rev.to_string())
            .set("hash", &hash.0)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(content)
            .map_err(map_err)?;
        read_json(resp)
    }

    fn delete(&self, path: &VaultPath, base_rev: Rev) -> TransportResult<PutOutcome> {
        let url = self.file_url(path);
        let resp = self
            .authed(self.agent.delete(&url))
            .set("base-rev", &base_rev.to_string())
            .call()
            .map_err(map_err)?;
        read_json(resp)
    }

    fn ack(&self, cursor: Seq) -> TransportResult<()> {
        let url = format!("{}/ack", self.vault_base());
        let body = serde_json::json!({ "cursor": cursor }).to_string();
        self.authed(self.agent.post(&url))
            .set("Content-Type", "application/json")
            .send_string(&body)
            .map_err(map_err)?;
        Ok(())
    }
}

/// Read a JSON response body into a protocol type. The server and engine share
/// these types (`kairn_sync::types`), so a shape mismatch is a real protocol
/// error, never expected drift.
fn read_json<T: serde::de::DeserializeOwned>(resp: ureq::Response) -> TransportResult<T> {
    let body = resp
        .into_string()
        .map_err(|e| TransportError::Network(e.to_string()))?;
    serde_json::from_str(&body).map_err(|e| TransportError::Protocol(format!("bad json body: {e}")))
}

/// Map a `ureq` failure onto the transport's error vocabulary. A non-2xx status
/// is a `ureq::Error::Status`; auth and not-found get their own variants so the
/// cycle can react (spec §10, §11), everything else is a protocol error carrying
/// the server's message. Connection/timeout failures are `Network`.
fn map_err(e: ureq::Error) -> TransportError {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            match code {
                401 | 403 => TransportError::Auth,
                404 => TransportError::NotFound,
                _ => TransportError::Protocol(format!("HTTP {code}: {}", body.trim())),
            }
        }
        ureq::Error::Transport(t) => TransportError::Network(t.to_string()),
    }
}

/// Percent-encode a vault path for a URL, keeping `/` as the path separator
/// (the server route captures the rest of the path as a wildcard) and encoding
/// everything else outside the RFC 3986 unreserved set, so spaces and unicode in
/// note names round-trip. Vault paths never contain `.`/`..`/`/` components
/// beyond the separators (spec §2, `VaultPath::is_safe`), so this is lossless.
fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for b in seg.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_segments_but_keeps_separators() {
        assert_eq!(encode_path("Calendar/20260808.md"), "Calendar/20260808.md");
        assert_eq!(encode_path("Notes/a b.md"), "Notes/a%20b.md");
        assert_eq!(encode_path("Notes/café.md"), "Notes/caf%C3%A9.md");
    }

    #[test]
    fn strips_trailing_slash_from_base() {
        let t = HttpTransport::new("http://host:8787/", "default", "tok");
        assert_eq!(t.vault_base(), "http://host:8787/v1/vaults/default");
    }

    /// A round trip against a *real, running* server, off by default. Point it at
    /// one by setting `KAIRN_SYNC_LIVE_URL`, `KAIRN_SYNC_LIVE_TOKEN`, and
    /// optionally `KAIRN_SYNC_LIVE_VAULT` (defaults to `default`); enroll a
    /// throwaway device first and revoke it after. It writes then deletes a
    /// uniquely named probe file, so it leaves no head behind. Hermetic coverage
    /// lives in the server crate's `http_transport` test; this only proves the
    /// deployed binary and the tailnet transport agree.
    #[test]
    fn live_smoke() {
        let (url, token) = match (
            std::env::var("KAIRN_SYNC_LIVE_URL"),
            std::env::var("KAIRN_SYNC_LIVE_TOKEN"),
        ) {
            (Ok(u), Ok(t)) => (u, t),
            _ => {
                eprintln!("live_smoke skipped (set KAIRN_SYNC_LIVE_URL + KAIRN_SYNC_LIVE_TOKEN)");
                return;
            }
        };
        let vault = std::env::var("KAIRN_SYNC_LIVE_VAULT").unwrap_or_else(|_| "default".into());
        let t = HttpTransport::new(url, vault, token);

        let path = VaultPath::new(format!("_smoke-{}.md", std::process::id()));
        let body = b"kairn sync live smoke\n";
        let hash = crate::hash::hash_bytes(body);

        // Read path (auth + JSON over the real socket).
        let before = t.changes(0, 500).expect("changes should succeed");

        // Write path against the deployed store.
        let put = t.put_blob(&path, 0, &hash, body).expect("put should succeed");
        let rev = match put {
            PutOutcome::Accepted { rev, .. } => rev,
            PutOutcome::Conflict { .. } => panic!("probe path unexpectedly existed"),
        };
        assert_eq!(t.get_blob(&path, None).expect("get should succeed"), body);

        // The probe now shows up in the journal.
        let after = t.changes(0, 500).expect("changes should succeed");
        assert!(after.cursor > before.cursor);

        // Clean up: tombstone the probe so no head is left on the live server.
        t.delete(&path, rev).expect("delete should succeed");
        assert!(matches!(
            t.get_blob(&path, None),
            Err(TransportError::NotFound)
        ));
        eprintln!("live_smoke ok: round-tripped and cleaned up {path}");
    }
}
