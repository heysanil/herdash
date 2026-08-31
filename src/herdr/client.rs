//! NDJSON client for the herdr socket API.
//!
//! The connection is generic over any async stream so tests can drive it with
//! `tokio::io::duplex()` instead of a real socket.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::types::{ReadEnvelope, Snapshot, SnapshotEnvelope};

/// A request/response connection speaking newline-delimited JSON.
///
/// Requests are strictly sequential: each [`Self::request`] writes one line and
/// reads lines until it sees one whose `id` matches. Out-of-band lines are
/// skipped, which keeps the client correct even if event subscriptions are
/// added later.
pub struct Connection<S> {
    stream: BufReader<S>,
    next_id: u64,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub fn new(stream: S) -> Self {
        Self { stream: BufReader::new(stream), next_id: 0 }
    }

    /// Issue one request and await its matching response.
    pub async fn request<P: Serialize, R: DeserializeOwned>(
        &mut self,
        method: &str,
        params: P,
    ) -> Result<R> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        let line = serde_json::to_string(&json!({
            "id": &id,
            "method": method,
            "params": params,
        }))?;

        self.stream.get_mut().write_all(line.as_bytes()).await?;
        self.stream.get_mut().write_all(b"\n").await?;
        self.stream.get_mut().flush().await?;

        loop {
            let mut buf = String::new();
            let n = self.stream.read_line(&mut buf).await?;
            if n == 0 {
                bail!("herdr closed the connection during `{method}`");
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(trimmed)
                .with_context(|| format!("herdr sent invalid JSON: {trimmed}"))?;

            // Skip anything that is not our response.
            if v.get("id").and_then(|i| i.as_str()) != Some(id.as_str()) {
                continue;
            }
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(|c| c.as_str()).unwrap_or("error");
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
                bail!("herdr `{method}` failed [{code}]: {msg}");
            }
            let result = v.get("result").cloned().unwrap_or(serde_json::Value::Null);
            return serde_json::from_value(result)
                .with_context(|| format!("unexpected shape for `{method}` result"));
        }
    }

    /// Whole-session state: every agent and workspace herdr knows about.
    pub async fn snapshot(&mut self) -> Result<Snapshot> {
        let env: SnapshotEnvelope = self.request("session.snapshot", json!({})).await?;
        Ok(env.snapshot)
    }

    /// Recent transcript for the agent occupying `pane_id`.
    ///
    /// Uses `recent_unwrapped` (underscored — the hyphenated CLI spelling is
    /// rejected on the wire), which rejoins soft wraps. Reading does not mark
    /// the agent's tab as seen, so polling never flips `done` back to `idle`.
    pub async fn read_agent(&mut self, pane_id: &str, lines: u32) -> Result<String> {
        let env: ReadEnvelope = self
            .request(
                "agent.read",
                json!({
                    "target": pane_id,
                    "source": "recent_unwrapped",
                    "lines": lines,
                    "strip_ansi": true,
                    "format": "text",
                }),
            )
            .await?;
        Ok(env.read.text)
    }

    /// Jump herdr's UI to the pane hosting this agent.
    pub async fn focus_agent(&mut self, pane_id: &str) -> Result<()> {
        let _: serde_json::Value =
            self.request("agent.focus", json!({ "target": pane_id })).await?;
        Ok(())
    }
}

/// Connect to a herdr server over its Unix socket.
pub async fn connect_unix(path: &Path) -> Result<Connection<UnixStream>> {
    let stream = UnixStream::connect(path)
        .await
        .with_context(|| format!("could not connect to herdr socket at {}", path.display()))?;
    Ok(Connection::new(stream))
}
