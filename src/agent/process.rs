use super::protocol::{parse_notification, parse_response_error, RhoEvent, RequestKind};
use crate::makepad_widgets::SignalToUI;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;

pub(super) enum RhoOutput {
    Stdout(String),
    Stderr(String),
    StdoutClosed,
}

pub struct RhoAgent {
    child: Child,
    stdin: ChildStdin,
    receiver: mpsc::Receiver<RhoOutput>,
    next_id: u64,
    pending: HashMap<u64, RequestKind>,
}

impl Drop for RhoAgent {
    fn drop(&mut self) {
        // Ensure the subprocess is terminated — dropping Child alone does
        // NOT kill the process on Windows or Unix, it only closes the handle.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl RhoAgent {
    /// Spawn `rho` (from `RHO_PATH`, else PATH) with piped stdio and start the
    /// reader threads. Returns Err if the binary can't be launched.
    pub fn spawn() -> Result<Self, String> {
        let program = std::env::var("RHO_PATH").unwrap_or_else(|_| "rho".to_string());
        let mut child = Command::new(&program)
            .arg("--accept-external-provider")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {}", program, e))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;

        let (tx, rx) = mpsc::channel::<RhoOutput>();

        let tx_out = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if tx_out.send(RhoOutput::Stdout(line)).is_err() {
                            break;
                        }
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => break,
                }
            }
            let _ = tx_out.send(RhoOutput::StdoutClosed);
            SignalToUI::set_ui_signal();
        });

        let tx_err = tx;
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => {
                        if tx_err.send(RhoOutput::Stderr(line)).is_err() {
                            break;
                        }
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            receiver: rx,
            next_id: 0,
            pending: HashMap::new(),
        })
    }

    /// Drain queued subprocess output, parsing stdout lines into events.
    pub fn drain(&mut self) -> Vec<RhoEvent> {
        let mut events = Vec::new();
        while let Ok(out) = self.receiver.try_recv() {
            match out {
                RhoOutput::Stdout(line) => {
                    if let Some(ev) = self.parse_stdout(&line) {
                        events.push(ev);
                    }
                }
                RhoOutput::Stderr(line) => eprintln!("[rho] {}", line),
                RhoOutput::StdoutClosed => events.push(RhoEvent::Closed),
            }
        }
        events
    }

    /// Parse one stdout line: a JSON-RPC response (has `id`, matched against
    /// `pending`) or a notification (has `method`).
    fn parse_stdout(&mut self, line: &str) -> Option<RhoEvent> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if let Some(id) = v.get("id").and_then(|x| x.as_u64()) {
            let kind = self.pending.remove(&id)?;
            if v.get("error").is_some() {
                return Some(RhoEvent::RequestError {
                    kind,
                    error: parse_response_error(&v),
                });
            }
            return Some(RhoEvent::Response {
                kind,
                result: v.get("result").cloned().unwrap_or(serde_json::Value::Null),
            });
        }
        parse_notification(&v)
    }

    /// Write a JSON-RPC request. If `kind` is set, the response is tracked and
    /// surfaced as `Response`/`RequestError`; otherwise fire-and-forget.
    fn write_jsonrpc(
        &mut self,
        method: &str,
        params: serde_json::Value,
        kind: Option<RequestKind>,
    ) -> Result<(), String> {
        self.next_id += 1;
        let id = self.next_id;
        if let Some(k) = kind {
            self.pending.insert(id, k);
        }
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });
        writeln!(self.stdin, "{}", line).map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn fire(&mut self, method: &str, params: serde_json::Value) -> Result<(), String> {
        self.write_jsonrpc(method, params, None)
    }
    fn request(
        &mut self,
        kind: RequestKind,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        self.write_jsonrpc(method, params, Some(kind))
    }

    pub fn prompt(&mut self, message: &str, steer: bool) -> Result<(), String> {
        self.fire("prompt", serde_json::json!({ "message": message, "steer": steer }))
    }
    pub fn abort(&mut self) -> Result<(), String> {
        self.fire("abort", serde_json::json!({}))
    }
    pub fn get_state(&mut self) -> Result<(), String> {
        self.request(RequestKind::GetState, "getState", serde_json::json!({}))
    }
    pub fn list_models(&mut self) -> Result<(), String> {
        self.request(RequestKind::ListModels, "listModels", serde_json::json!({}))
    }
    pub fn list_providers(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::ListProviders,
            "listProviders",
            serde_json::json!({}),
        )
    }
    pub fn list_sessions(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::ListSessions,
            "listSessions",
            serde_json::json!({}),
        )
    }
    pub fn set_model(&mut self, model: &str) -> Result<(), String> {
        self.request(
            RequestKind::SetModel,
            "setModel",
            serde_json::json!({ "model": model }),
        )
    }
    pub fn resume_session(&mut self, path: &str) -> Result<(), String> {
        self.request(
            RequestKind::ResumeSession,
            "resumeSession",
            serde_json::json!({ "path": path }),
        )
    }
    pub fn get_session_stats(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::GetSessionStats,
            "getSessionStats",
            serde_json::json!({}),
        )
    }
    pub fn reload_extensions(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::ReloadExtensions,
            "reloadExtensions",
            serde_json::json!({}),
        )
    }
    pub fn list_extensions(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::ListExtensions,
            "listExtensions",
            serde_json::json!({}),
        )
    }
    pub fn approval_response(
        &mut self,
        approved: bool,
        message: Option<String>,
    ) -> Result<(), String> {
        let params = match message {
            Some(m) => serde_json::json!({ "approved": approved, "message": m }),
            None => serde_json::json!({ "approved": approved }),
        };
        self.fire("approvalResponse", params)
    }
}