//! Chrome DevTools Protocol client for the Codex desktop app (port of
//! `cdp.mjs`). Loopback-only, verified targets, no app files touched.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::{Result, ThemeEngineError};

const OPEN_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const LIST_TIMEOUT: Duration = Duration::from_secs(2);
const VERSION_TIMEOUT: Duration = Duration::from_millis(1200);

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Cdp(message.into())
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "type")]
    pub target_type: String,
    #[serde(default)]
    pub web_socket_debugger_url: String,
}

/// Reject anything that is not `ws://<loopback>:<port>/...` — the daemon must
/// never follow a debugger URL onto the network.
pub fn validated_debugger_url(target: &TargetInfo, port: u16) -> Result<String> {
    let url = reqwest::Url::parse(&target.web_socket_debugger_url)
        .map_err(|e| err(format!("invalid CDP WebSocket URL: {e}")))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1") | Some("localhost") | Some("[::1]"))
        || url.host_str() == Some("::1");
    if url.scheme() != "ws" || !loopback || url.port() != Some(port) {
        return Err(err(format!(
            "rejected non-loopback CDP WebSocket URL: {}",
            url
        )));
    }
    Ok(url.to_string())
}

#[derive(Debug, Clone)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<std::result::Result<Value, String>>>>>;

/// One WebSocket session against a renderer target. Cheap to clone; the
/// underlying socket closes when the reader task sees EOF or `close()` runs.
#[derive(Clone)]
pub struct CdpSession {
    pub target: TargetInfo,
    tx: mpsc::UnboundedSender<Message>,
    pending: Pending,
    events: broadcast::Sender<CdpEvent>,
    next_id: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl CdpSession {
    /// Connect, then enable the Runtime and Page domains (the studio's
    /// session-open contract).
    pub async fn connect(target: TargetInfo, port: u16) -> Result<Self> {
        let url = validated_debugger_url(&target, port)?;
        let (stream, _response) = tokio::time::timeout(OPEN_TIMEOUT, connect_async(&url))
            .await
            .map_err(|_| err("CDP WebSocket open timed out"))?
            .map_err(|e| err(format!("CDP WebSocket open failed: {e}")))?;
        let (mut sink, mut source) = stream.split();

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let (events, _) = broadcast::channel(64);
        let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

        // Writer: serialized command frames out.
        let writer_closed = closed.clone();
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                if sink.send(message).await.is_err() {
                    writer_closed.store(true, Ordering::SeqCst);
                    break;
                }
            }
            let _ = sink.close().await;
        });

        // Reader: route responses to their waiters, broadcast events.
        let reader_pending = pending.clone();
        let reader_closed = closed.clone();
        let reader_events = events.clone();
        tokio::spawn(async move {
            while let Some(frame) = source.next().await {
                let Ok(Message::Text(text)) = frame else {
                    if frame.is_err() {
                        break;
                    }
                    continue;
                };
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(id) = message.get("id").and_then(|v| v.as_u64()) {
                    let waiter = reader_pending.lock().expect("pending lock").remove(&id);
                    if let Some(waiter) = waiter {
                        let outcome = match message.get("error") {
                            Some(error) => Err(format!(
                                "{} ({})",
                                error.get("message").and_then(|m| m.as_str()).unwrap_or("?"),
                                error.get("code").and_then(|c| c.as_i64()).unwrap_or(0)
                            )),
                            None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = waiter.send(outcome);
                    }
                    continue;
                }
                if let Some(method) = message.get("method").and_then(|v| v.as_str()) {
                    let _ = reader_events.send(CdpEvent {
                        method: method.to_string(),
                        params: message.get("params").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            reader_closed.store(true, Ordering::SeqCst);
            let mut pending = reader_pending.lock().expect("pending lock");
            for (_, waiter) in pending.drain() {
                let _ = waiter.send(Err("CDP socket closed".to_string()));
            }
        });

        let session = Self {
            target,
            tx,
            pending,
            events,
            next_id: Arc::new(AtomicU64::new(1)),
            closed,
        };
        session.send("Runtime.enable", json!({})).await?;
        session.send("Page.enable", json!({})).await?;
        Ok(session)
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Subscribe to protocol events (e.g. `Page.loadEventFired`).
    pub fn events(&self) -> broadcast::Receiver<CdpEvent> {
        self.events.subscribe()
    }

    pub async fn send(&self, method: &str, params: Value) -> Result<Value> {
        if self.closed() {
            return Err(err("CDP session is closed"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (waiter_tx, waiter_rx) = oneshot::channel();
        self.pending
            .lock()
            .expect("pending lock")
            .insert(id, waiter_tx);
        let frame = json!({ "id": id, "method": method, "params": params });
        if self.tx.send(Message::text(frame.to_string())).is_err() {
            self.pending.lock().expect("pending lock").remove(&id);
            return Err(err("CDP session is closed"));
        }
        match tokio::time::timeout(COMMAND_TIMEOUT, waiter_rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(error))) => Err(err(error)),
            Ok(Err(_)) => Err(err("CDP socket closed")),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(err(format!("CDP command timed out: {method}")))
            }
        }
    }

    /// Evaluate an expression in the renderer, unwrapping by-value results and
    /// surfacing renderer exceptions as errors.
    pub async fn evaluate(&self, expression: &str) -> Result<Value> {
        let result = self
            .send(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": false,
                }),
            )
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            let detail = details
                .pointer("/exception/description")
                .or_else(|| details.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown renderer exception");
            return Err(err(format!("renderer evaluation failed: {detail}")));
        }
        Ok(result.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Message::Close(None));
    }
}

fn loopback_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(timeout)
        .build()
        .map_err(|e| err(format!("http client: {e}")))
}

/// Enumerate verified `app://` page targets, main window first (auxiliary
/// prewarm/panel targets carry query-string routes and sort after it).
pub async fn list_app_targets(port: u16) -> Result<Vec<TargetInfo>> {
    let client = loopback_client(LIST_TIMEOUT)?;
    let body = client
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .send()
        .await
        .map_err(|e| err(format!("target list failed: {e}")))?
        .error_for_status()
        .map_err(|e| err(format!("target list failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| err(format!("target list read failed: {e}")))?;
    let targets: Vec<TargetInfo> = serde_json::from_slice(&body)
        .map_err(|e| err(format!("target list parse failed: {e}")))?;
    let mut filtered: Vec<TargetInfo> = targets
        .into_iter()
        .filter(|t| {
            t.target_type == "page"
                && t.url.starts_with("app://")
                && !t.web_socket_debugger_url.is_empty()
                && validated_debugger_url(t, port).is_ok()
        })
        .collect();
    filtered.sort_by_key(|t| t.url.contains('?'));
    Ok(filtered)
}

/// Whether a CDP HTTP endpoint answers on the port (does NOT prove it is
/// Codex — pair with a shell probe before injecting).
pub async fn cdp_http_ready(port: u16) -> bool {
    let Ok(client) = loopback_client(VERSION_TIMEOUT) else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://127.0.0.1:{port}/json/version"))
        .send()
        .await
    else {
        return false;
    };
    let Ok(bytes) = response.bytes().await else {
        return false;
    };
    let Ok(body) = serde_json::from_slice::<Value>(&bytes) else {
        return false;
    };
    body.get("webSocketDebuggerUrl").is_some_and(Value::is_string)
        || body.get("Browser").is_some_and(Value::is_string)
}

/// Renderer-shell probe: accepts pages exposing the native Codex shell
/// markers, or — for full-screen routes like Settings where the shell
/// unmounts — an `app://` page whose title still identifies Codex.
pub const PROBE_EXPRESSION: &str = r#"(() => {
    const markers = {
      shell: Boolean(document.querySelector('main.main-surface')),
      sidebar: Boolean(document.querySelector('.app-shell-left-panel')),
      composer: Boolean(document.querySelector('.composer-surface-chrome')),
      main: Boolean(document.querySelector('[role="main"]')),
    };
    const shellMatch = markers.shell && markers.sidebar && (markers.composer || markers.main);
    const titleMatch = /codex|chatgpt/i.test(document.title) && location.protocol === 'app:';
    return {
      title: document.title,
      href: location.href,
      markers,
      codex: shellMatch || titleMatch,
    };
  })()"#;

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeResult {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub href: String,
    #[serde(default)]
    pub codex: bool,
}

pub async fn probe_session(session: &CdpSession) -> Result<ProbeResult> {
    let value = session.evaluate(PROBE_EXPRESSION).await?;
    serde_json::from_value(value).map_err(|e| err(format!("probe parse failed: {e}")))
}

pub struct ConnectedTarget {
    pub session: CdpSession,
    pub probe: ProbeResult,
}

/// Connect every verified Codex renderer on the port, retrying until the
/// deadline (Codex needs a moment after launch before targets appear).
pub async fn connect_codex_targets(
    port: u16,
    timeout: Duration,
) -> Result<Vec<ConnectedTarget>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_error: Option<String> = None;
    loop {
        match list_app_targets(port).await {
            Ok(targets) => {
                let mut connected = Vec::new();
                for target in targets {
                    match CdpSession::connect(target, port).await {
                        Ok(session) => match probe_session(&session).await {
                            Ok(probe) if probe.codex => {
                                connected.push(ConnectedTarget { session, probe });
                            }
                            Ok(_) => session.close(),
                            Err(error) => {
                                session.close();
                                last_error = Some(error.to_string());
                            }
                        },
                        Err(error) => last_error = Some(error.to_string()),
                    }
                }
                if !connected.is_empty() {
                    return Ok(connected);
                }
                last_error
                    .get_or_insert_with(|| "no page matched the Codex shell markers".to_string());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(err(format!(
                "no verified Codex renderer on 127.0.0.1:{port}: {}",
                last_error.unwrap_or_else(|| "timed out".to_string())
            )));
        }
        tokio::time::sleep(Duration::from_millis(350)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: &str) -> TargetInfo {
        TargetInfo {
            id: "t1".into(),
            title: "Codex".into(),
            url: "app://-/".into(),
            target_type: "page".into(),
            web_socket_debugger_url: url.into(),
        }
    }

    #[test]
    fn debugger_url_must_be_loopback_ws_on_the_expected_port() {
        assert!(validated_debugger_url(&target("ws://127.0.0.1:9345/devtools/page/1"), 9345).is_ok());
        assert!(validated_debugger_url(&target("ws://localhost:9345/x"), 9345).is_ok());
        // Wrong port, wrong scheme, non-loopback host: all rejected.
        assert!(validated_debugger_url(&target("ws://127.0.0.1:9346/x"), 9345).is_err());
        assert!(validated_debugger_url(&target("wss://127.0.0.1:9345/x"), 9345).is_err());
        assert!(validated_debugger_url(&target("ws://192.168.1.4:9345/x"), 9345).is_err());
        assert!(validated_debugger_url(&target("http://127.0.0.1:9345/x"), 9345).is_err());
    }
}
