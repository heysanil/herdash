//! NDJSON client for the herdr socket API.
//!
//! # Connection model
//!
//! herdr answers **one request per connection** and then closes the socket.
//! Verified against herdr 0.8.2: a second request on the same stream fails
//! with `EPIPE`, and pipelined requests receive one response followed by EOF.
//! (`events.subscribe` is the exception — it converts the connection into an
//! event stream — but herdash polls rather than subscribing, because the
//! per-pane subscriptions it would need churn as panes come and go.)
//!
//! So [`Client`] opens a fresh connection per call. On a Unix socket that
//! costs tens of microseconds, which is irrelevant next to a 1 Hz poll, and it
//! removes an entire reconnect state machine: there is no long-lived
//! connection to lose.
//!
//! [`Connection`] remains generic over any async stream so the framing can be
//! tested with `tokio::io::duplex()` instead of a real socket.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use super::types::{ReadEnvelope, ReadPayload, Snapshot, SnapshotEnvelope};

/// Upper bound on a status poll.
///
/// Without a bound, a connected-but-unresponsive server would wedge the
/// poller forever and the dashboard would silently freeze on stale data while
/// claiming to be connected. Kept short because a snapshot is small and the
/// poll repeats every second anyway.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound on a transcript read.
///
/// Reads are background work and can be far larger than a snapshot — up to a
/// few hundred lines of pane history from a server that may be busy driving
/// several agents. Observed live: a 200-line read occasionally exceeded 5s,
/// which needlessly burned a summary and pushed that agent into backoff.
pub const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Why a herdr call failed.
///
/// The distinction matters: a transport failure means herdr is gone and the
/// header should say so, while a protocol or remote failure means herdr is
/// alive and answering — just not usefully — so the dashboard keeps polling.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not connect to herdr socket at {path}: {source}")]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("herdr connection failed during `{method}`: {source}")]
    Io {
        method: String,
        source: std::io::Error,
    },

    #[error("herdr closed the connection during `{method}`")]
    Eof { method: String },

    #[error("herdr `{method}` timed out after {}s", timeout.as_secs())]
    Timeout { method: String, timeout: Duration },

    #[error("herdr `{method}` failed [{code}]: {message}")]
    Remote {
        method: String,
        code: String,
        message: String,
    },

    #[error("herdr sent an unexpected response for `{method}`: {detail}")]
    Protocol { method: String, detail: String },
}

impl ClientError {
    /// True when herdr itself is unreachable, as opposed to answering badly.
    ///
    /// Only a transport failure should flip the UI to "reconnecting"; a
    /// malformed payload or a rejected method means the server is up.
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::Connect { .. } | Self::Io { .. } | Self::Eof { .. } | Self::Timeout { .. }
        )
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;

/// A single request/response exchange speaking newline-delimited JSON.
///
/// Lines whose `id` does not match are skipped, so an interleaved event
/// cannot be mistaken for a response.
#[derive(Debug)]
pub struct Connection<S> {
    stream: BufReader<S>,
    next_id: u64,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream: BufReader::new(stream),
            next_id: 0,
        }
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
        }))
        .map_err(|e| ClientError::Protocol {
            method: method.into(),
            detail: e.to_string(),
        })?;

        let io = |e: std::io::Error| ClientError::Io {
            method: method.to_string(),
            source: e,
        };
        self.stream
            .get_mut()
            .write_all(line.as_bytes())
            .await
            .map_err(io)?;
        self.stream.get_mut().write_all(b"\n").await.map_err(io)?;
        self.stream.get_mut().flush().await.map_err(io)?;

        loop {
            let mut buf = String::new();
            let n = self.stream.read_line(&mut buf).await.map_err(io)?;
            if n == 0 {
                return Err(ClientError::Eof {
                    method: method.to_string(),
                });
            }
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: serde_json::Value =
                serde_json::from_str(trimmed).map_err(|e| ClientError::Protocol {
                    method: method.to_string(),
                    detail: format!("invalid JSON ({e}): {}", truncate(trimmed, 160)),
                })?;

            // Skip anything that is not our response.
            if v.get("id").and_then(|i| i.as_str()) != Some(id.as_str()) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(ClientError::Remote {
                    method: method.to_string(),
                    code: err
                        .get("code")
                        .and_then(|c| c.as_str())
                        .unwrap_or("error")
                        .to_string(),
                    message: err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or_default()
                        .to_string(),
                });
            }
            let result = v.get("result").cloned().unwrap_or(serde_json::Value::Null);
            return serde_json::from_value(result).map_err(|e| ClientError::Protocol {
                method: method.to_string(),
                detail: format!("unexpected result shape: {e}"),
            });
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Talks to a herdr server, opening one connection per request.
///
/// Cheap to clone, and every method takes `&self`, so tasks can issue calls
/// concurrently without sharing a socket.
#[derive(Debug, Clone)]
pub struct Client {
    socket: PathBuf,
    timeout: Duration,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
            timeout: REQUEST_TIMEOUT,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Connect, send one request, read one response, close.
    pub async fn request<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R> {
        self.request_within(method, params, self.timeout).await
    }

    async fn request_within<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<R> {
        let fut = async {
            let stream =
                UnixStream::connect(&self.socket)
                    .await
                    .map_err(|source| ClientError::Connect {
                        path: self.socket.clone(),
                        source,
                    })?;
            Connection::new(stream).request(method, params).await
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| ClientError::Timeout {
                method: method.to_string(),
                timeout: self.timeout,
            })?
    }

    /// Whole-session state: every agent and workspace herdr knows about.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let env: SnapshotEnvelope = self.request("session.snapshot", json!({})).await?;
        Ok(env.snapshot)
    }

    /// Recent transcript for the agent occupying `pane_id`.
    ///
    /// Uses `recent_unwrapped` (underscored — the hyphenated CLI spelling is
    /// rejected on the wire), which rejoins soft wraps, and falls back to
    /// `visible` when herdr refuses.
    ///
    /// # Why the fallback exists
    ///
    /// A *working* agent usually renders on the terminal's alternate screen,
    /// which has no scrollback. herdr will not serve more rows than that
    /// viewport holds and returns `agent_not_idle` rather than truncating.
    /// Verified against herdr 0.8.2 on a 55-row pane: `recent_unwrapped` with
    /// `lines = 60` fails, `lines = 40` succeeds, and `visible` always works.
    ///
    /// Without this fallback the agents you most want summarised — the ones
    /// actively working — are the only ones that never get a summary.
    ///
    /// Returns the whole payload so the caller can record the revision the
    /// text actually came from, rather than the one the snapshot reported
    /// moments earlier.
    ///
    /// Reading does not mark the agent's tab as seen, so polling never flips
    /// a `done` agent back to `idle`.
    pub async fn read_agent(&self, pane_id: &str, lines: u32) -> Result<ReadPayload> {
        match self
            .read_source(pane_id, "recent_unwrapped", Some(lines))
            .await
        {
            Err(ClientError::Remote { code, .. }) if code == "agent_not_idle" => {
                // Viewport-bounded by definition, so this cannot overrun.
                self.read_source(pane_id, "visible", None).await
            }
            other => other,
        }
    }

    async fn read_source(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<u32>,
    ) -> Result<ReadPayload> {
        let mut params = serde_json::Map::new();
        params.insert("target".into(), json!(pane_id));
        params.insert("source".into(), json!(source));
        params.insert("strip_ansi".into(), json!(true));
        params.insert("format".into(), json!("text"));
        if let Some(lines) = lines {
            params.insert("lines".into(), json!(lines));
        }
        let env: ReadEnvelope = self
            .request("agent.read", serde_json::Value::Object(params))
            .await?;
        Ok(env.read)
    }

    /// Jump herdr's UI to the pane hosting this agent.
    pub async fn focus_agent(&self, pane_id: &str) -> Result<()> {
        let _: serde_json::Value = self
            .request("agent.focus", json!({ "target": pane_id }))
            .await?;
        Ok(())
    }
}
