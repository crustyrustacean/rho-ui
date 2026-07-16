pub use makepad_widgets;

use makepad_widgets::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::time::Instant;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::RwLock;
use std::thread;

app_main!(App);

/// Diagnostic tracing for the agent event stream. Silent by default; enable
/// with `RHO_UI_TRACE=1` to see every `RhoEvent` and `busy` transition.
macro_rules! trace {
    ($($arg:tt)*) => {
        if crate::trace_enabled() {
            eprintln!($($arg)*);
        }
    };
}

// ── Message model ──────────────────────────────────────────────────────────
// The conversation lives in a global: the ChatScroll widget reads it during
// draw, App mutates it in response to user actions. This mirrors the makepad
// `aichat` example's CHAT_DATA pattern.

#[derive(Clone, Debug)]
enum ChatBlock {
    /// A user-submitted message.
    User(String),
    /// A completed reasoning ("thinking") block with elapsed time.
    Reasoning { text: String, elapsed_secs: String },
    /// A streaming reasoning block (still thinking).
    ReasoningStreaming { text: String },
    /// A plain-text response from the agent.
    Response(String),
    /// A tool call block with status.
    ToolCall {
        name: String,
        args: String,
        status: ToolStatus,
        output: Option<String>,
    },
    /// A tool-approval prompt from the agent (blocks until we respond).
    Approval {
        tool: String,
        arguments: String,
        risk: String,
        resolution: ApprovalResolution,
    },
    /// A clickable previous-session entry (click to resume).
    SessionEntry { path: String, entries: u64 },
    /// A system/info message (e.g. "switched model", "resumed session").
    Info(String),
}

#[derive(Clone, Debug)]
enum ToolStatus {
    Pending,
    Success,
    Error,
    Denied,
}

#[derive(Clone, Debug)]
enum ApprovalResolution {
    Pending,
    Approved,
    Denied,
    Redirected(String),
}

static CHAT_BLOCKS: RwLock<Vec<ChatBlock>> = RwLock::new(Vec::new());

/// Models shown in the picker modal: (name, is_current).
static MODELS: RwLock<Vec<(String, bool)>> = RwLock::new(Vec::new());

// ── rho agent bridge ────────────────────────────────────────────────────────
// rho-coding-agent runs as a headless JSON-RPC 2.0 server over stdio. We spawn
// it as a child process, read stdout/stderr on background threads into an mpsc
// channel, and wake makepad's event loop with `SignalToUI::set_ui_signal()` —
// the same pattern makepad_ai's claude_code backend uses. On each `Event::Signal`
// tick we drain the channel, parse JSON-RPC notifications, and App maps them onto
// CHAT_BLOCKS.
enum RhoOutput {
    Stdout(String),
    Stderr(String),
    StdoutClosed,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
enum RequestKind {
    GetState,
    GetSessionStats,
    ListModels,
    ListProviders,
    ListSessions,
    SetModel,
    ResumeSession,
}

#[derive(Clone, Debug)]
enum RhoEvent {
    Ready,
    AgentStart,
    AgentEnd { reply: String, duration_ms: u64 },
    AgentError { error: String },
    MessageDelta { delta: String },
    ReasoningDelta { delta: String },
    StateChange { state: String },
    ToolCall { name: String, arguments: String },
    ToolResult { name: String, is_error: bool, output: String },
    ToolDenied { name: String },
    ApprovalRequest { tool: String, arguments: String, risk: String },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        cost: f64,
        context_used: u64,
        context_window: u64,
        utilization: u8,
    },
    Response { kind: RequestKind, result: serde_json::Value },
    RequestError { kind: RequestKind, error: String },
    Closed,
}

pub struct RhoAgent {
    _child: Child,
    stdin: ChildStdin,
    receiver: mpsc::Receiver<RhoOutput>,
    next_id: u64,
    pending: HashMap<u64, RequestKind>,
}

impl RhoAgent {
    /// Spawn `rho` (from `RHO_PATH`, else PATH) with piped stdio and start the
    /// reader threads. Returns Err if the binary can't be launched.
    fn spawn() -> Result<Self, String> {
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

        Ok(Self { _child: child, stdin, receiver: rx, next_id: 0, pending: HashMap::new() })
    }

    /// Drain queued subprocess output, parsing stdout lines into events.
    fn drain(&mut self) -> Vec<RhoEvent> {
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
            if let Some(err) = v.get("error") {
                return Some(RhoEvent::RequestError {
                    kind,
                    error: err
                        .get("message")
                        .and_then(|x| x.as_str())
                        .unwrap_or("request failed")
                        .to_string(),
                });
            }
            return Some(RhoEvent::Response {
                kind,
                result: v.get("result").cloned().unwrap_or(serde_json::Value::Null),
            });
        }
        let method = v.get("method")?.as_str()?;
        let p = v.get("params");
        Some(match method {
            "ready" => RhoEvent::Ready,
            "agent/start" => RhoEvent::AgentStart,
            "agent/end" => RhoEvent::AgentEnd {
                reply: jstr(p, "reply"),
                duration_ms: ju64(p, "durationMs"),
            },
            "agent/error" => RhoEvent::AgentError { error: jstr(p, "error") },
            "message/delta" => RhoEvent::MessageDelta { delta: jstr(p, "delta") },
            "reasoning/delta" => RhoEvent::ReasoningDelta { delta: jstr(p, "delta") },
            "state/change" => RhoEvent::StateChange { state: jstr(p, "state") },
            "tool/call" => RhoEvent::ToolCall {
                name: jstr(p, "name"),
                arguments: jstr(p, "arguments"),
            },
            "tool/result" => RhoEvent::ToolResult {
                name: jstr(p, "name"),
                is_error: jbool(p, "isError"),
                output: jstr(p, "output"),
            },
            "tool/denied" => RhoEvent::ToolDenied { name: jstr(p, "name") },
            "approval/request" => RhoEvent::ApprovalRequest {
                tool: jstr(p, "tool"),
                arguments: jstr(p, "arguments"),
                risk: jstr(p, "risk"),
            },
            "usage" => {
                let u = p.and_then(|p| p.get("usage"));
                let c = p.and_then(|p| p.get("context"));
                RhoEvent::Usage {
                    input_tokens: ju64(u, "inputTokens"),
                    output_tokens: ju64(u, "outputTokens"),
                    cached_tokens: ju64(u, "cachedTokens"),
                    cost: jf64(u, "cost"),
                    context_used: ju64(c, "estimatedUsed"),
                    context_window: ju64(c, "contextWindow"),
                    utilization: ju64(c, "utilizationPercent").min(255) as u8,
                }
            }
            _ => return None,
        })
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
    fn request(&mut self, kind: RequestKind, method: &str, params: serde_json::Value) -> Result<(), String> {
        self.write_jsonrpc(method, params, Some(kind))
    }

    fn prompt(&mut self, message: &str) -> Result<(), String> {
        self.fire("prompt", serde_json::json!({ "message": message }))
    }
    fn abort(&mut self) -> Result<(), String> {
        self.fire("abort", serde_json::json!({}))
    }
    fn get_state(&mut self) -> Result<(), String> {
        self.request(RequestKind::GetState, "getState", serde_json::json!({}))
    }
    fn list_models(&mut self) -> Result<(), String> {
        self.request(RequestKind::ListModels, "listModels", serde_json::json!({}))
    }
    fn list_providers(&mut self) -> Result<(), String> {
        self.request(RequestKind::ListProviders, "listProviders", serde_json::json!({}))
    }
    fn list_sessions(&mut self) -> Result<(), String> {
        self.request(RequestKind::ListSessions, "listSessions", serde_json::json!({}))
    }
    fn set_model(&mut self, model: &str) -> Result<(), String> {
        self.request(RequestKind::SetModel, "setModel", serde_json::json!({ "model": model }))
    }
    fn resume_session(&mut self, path: &str) -> Result<(), String> {
        self.request(RequestKind::ResumeSession, "resumeSession", serde_json::json!({ "path": path }))
    }
    fn get_session_stats(&mut self) -> Result<(), String> {
        self.request(RequestKind::GetSessionStats, "getSessionStats", serde_json::json!({}))
    }
    fn approval_response(&mut self, approved: bool, message: Option<String>) -> Result<(), String> {
        let params = match message {
            Some(m) => serde_json::json!({ "approved": approved, "message": m }),
            None => serde_json::json!({ "approved": approved }),
        };
        self.fire("approvalResponse", params)
    }
}

fn jstr(p: Option<&serde_json::Value>, k: &str) -> String {
    p.and_then(|p| p.get(k))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}
fn ju64(p: Option<&serde_json::Value>, k: &str) -> u64 {
    p.and_then(|p| p.get(k)).and_then(|x| x.as_u64()).unwrap_or(0)
}
fn jf64(p: Option<&serde_json::Value>, k: &str) -> f64 {
    p.and_then(|p| p.get(k)).and_then(|x| x.as_f64()).unwrap_or(0.0)
}
fn jbool(p: Option<&serde_json::Value>, k: &str) -> bool {
    p.and_then(|p| p.get(k)).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn format_secs(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// Whether `RHO_UI_TRACE` diagnostic tracing is on (parsed once, cached).
pub(crate) fn trace_enabled() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("RHO_UI_TRACE")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

// Streaming helpers: append into the trailing block of the right kind, creating
// it when the kind changes, so consecutive deltas accumulate into one block.
fn append_streaming_response(delta: &str) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    match blocks.last_mut() {
        Some(ChatBlock::Response(text)) => text.push_str(delta),
        _ => blocks.push(ChatBlock::Response(delta.to_string())),
    }
}
fn append_streaming_reasoning(delta: &str) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    match blocks.last_mut() {
        Some(ChatBlock::ReasoningStreaming { text }) => text.push_str(delta),
        _ => blocks.push(ChatBlock::ReasoningStreaming { text: delta.to_string() }),
    }
}
fn finalize_reasoning(elapsed_secs: String) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if let Some(last) = blocks.last_mut() {
        if let ChatBlock::ReasoningStreaming { text } = last {
            let owned = std::mem::take(text);
            *last = ChatBlock::Reasoning { text: owned, elapsed_secs };
        }
    }
}

/// Finalize the most recent pending ToolCall with `name` (rho's loop is
/// sequential, so there's at most one in flight per name). If none is found,
/// push a finalized block as a defensive fallback.
fn finalize_tool_call(name: &str, status: ToolStatus, output: Option<String>) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    for block in blocks.iter_mut().rev() {
        if let ChatBlock::ToolCall { name: n, status: st, output: out, .. } = block {
            if n == name && matches!(st, ToolStatus::Pending) {
                *st = status;
                *out = output;
                return;
            }
        }
    }
    drop(blocks);
    CHAT_BLOCKS.write().unwrap().push(ChatBlock::ToolCall {
        name: name.to_string(),
        args: String::new(),
        status,
        output,
    });
}

// ── ChatScroll: a PortalList-driven scrollback ──────────────────────────────
// Each ChatBlock variant maps to a named item template declared in the script
// below (User / Thought / Response / ToolDone / ToolRun / ToolFail / Info).
// During draw we walk the visible item range and fill each item's labels from
// the matching block.
//
// Auto-scroll is handled natively by PortalList, which is why this replaces the
// old Splash-based scrollback:
//   * `auto_tail: true` keeps the view pinned to the bottom *while the user is
//     already there*, and leaves it alone when they've scrolled up — so reading
//     history is never disrupted by incoming content.
//   * on send we call `set_first_id_and_scroll(last, 0)` + `set_tail_range(true)`
//     for an explicit "jump to newest" the instant a message is added.
// The Splash approach rebuilt the whole widget tree on every change, so it
// could neither preserve scroll position nor (it turned out) jump to the bottom
// — its programmatic set_scroll_pos was a silent no-op against the deref'd
// inner View, and even if it had worked the view was recreated each time.

#[derive(Script, ScriptHook, Widget)]
pub struct ChatScroll {
    #[deref]
    view: View,
}

impl Widget for ChatScroll {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Snapshot the blocks so we don't hold the lock across the draw.
        let blocks = CHAT_BLOCKS.read().unwrap().clone();

        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, blocks.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    if let Some(block) = blocks.get(item_id) {
                        match block {
                            ChatBlock::User(text) => {
                                let w = list.item(cx, item_id, id!(User));
                                w.label(cx, ids!(msg)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Reasoning { text, elapsed_secs } => {
                                let w = list.item(cx, item_id, id!(Thought));
                                w.label(cx, ids!(head))
                                    .set_text(cx, &format!("? thought · {}", elapsed_secs));
                                w.label(cx, ids!(body)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::ReasoningStreaming { text } => {
                                let w = list.item(cx, item_id, id!(Thought));
                                w.label(cx, ids!(head)).set_text(cx, "? thinking");
                                w.label(cx, ids!(body)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Response(text) => {
                                let w = list.item(cx, item_id, id!(Response));
                                w.markdown(cx, ids!(msg)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::ToolCall { name, args, status, output } => {
                                let (template, status_text) = match status {
                                    ToolStatus::Success => (id!(ToolDone), "✓ done"),
                                    ToolStatus::Pending => (id!(ToolRun), "⟳ running"),
                                    ToolStatus::Error => (id!(ToolFail), "✗ failed"),
                                    ToolStatus::Denied => (id!(ToolRun), "⊘ denied"),
                                };
                                let w = list.item(cx, item_id, template);
                                w.label(cx, ids!(head))
                                    .set_text(cx, &format!("{} {} {}", name, args, status_text));
                                match output {
                                    Some(out) => {
                                        w.widget(cx, ids!(out)).set_visible(cx, true);
                                        w.label(cx, ids!(out)).set_text(cx, out);
                                    }
                                    None => {
                                        w.widget(cx, ids!(out)).set_visible(cx, false);
                                    }
                                }
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Info(text) => {
                                let w = list.item(cx, item_id, id!(Info));
                                w.label(cx, ids!(msg)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Approval { tool, arguments, risk, resolution } => {
                                let w = list.item(cx, item_id, id!(Approval));
                                w.label(cx, ids!(head))
                                    .set_text(cx, &format!("{} {} ({})", tool, arguments, risk));
                                let (show_actions, resolved_text) = match resolution {
                                    ApprovalResolution::Pending => (true, None),
                                    ApprovalResolution::Approved => {
                                        (false, Some("\u{2713} approved".to_string()))
                                    }
                                    ApprovalResolution::Denied => {
                                        (false, Some("\u{2717} denied".to_string()))
                                    }
                                    ApprovalResolution::Redirected(msg) => {
                                        (false, Some(format!("\u{21aa} redirected: {}", msg)))
                                    }
                                };
                                w.widget(cx, ids!(buttons)).set_visible(cx, show_actions);
                                w.widget(cx, ids!(redirect_row)).set_visible(cx, show_actions);
                                w.widget(cx, ids!(resolved)).set_visible(cx, resolved_text.is_some());
                                if let Some(t) = resolved_text {
                                    w.label(cx, ids!(resolved)).set_text(cx, &t);
                                }
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::SessionEntry { path, entries } => {
                                let w = list.item(cx, item_id, id!(SessionEntry));
                                w.button(cx, ids!(btn))
                                    .set_text(cx, &format!("\u{21bb} {} ({} entries)", path, entries));
                                w.draw_all_unscoped(cx);
                            }
                        }
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ── ModelList: a PortalList of models for the picker modal ──────────────────
// Mirrors ChatScroll: snapshots the global MODELS during draw. App mutates
// MODELS (via sync_model_list) and redraws, so the modal's list stays in sync.
#[derive(Script, ScriptHook, Widget)]
pub struct ModelList {
    #[deref]
    view: View,
}

impl Widget for ModelList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let models = MODELS.read().unwrap().clone();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, models.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    if let Some((name, is_current)) = models.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        let label = if *is_current {
                            format!("\u{2713} {}", name)
                        } else {
                            name.clone()
                        };
                        w.button(cx, ids!(pick)).set_text(cx, &label);
                        w.draw_all_unscoped(cx);
                    }
                }
            }
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }
}

// ── The UI, in Script DSL ───────────────────────────────────────────────────
// Layout (top → bottom):
//   1. Title bar (Fit) — "rho" branding + subtitle.
//   2. Menu bar (Fit) — row of buttons: Session, Model, Resume, Providers,
//      Abort, Help, Quit.
//   3. Output scrollback (Fill) — a ChatScroll (PortalList) fed from CHAT_BLOCKS.
//   4. Working line (Fit) — spinner + "Working" + elapsed + activity.
//   5. Input box (Fit, 2 lines) — bordered multi-line text input.
//   6. Footer (Fit, 2 lines) — cwd + git branch, then token/cost/context stats
//      (left) + model name (right).
script_mod! {
    use mod.prelude.widgets.*

    // ChatScroll wraps a PortalList whose direct children are item templates.
    let ChatScroll = #(ChatScroll::register_widget(vm)) {
        width: Fill
        height: Fill
        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            auto_tail: true
            smooth_tail: true
            drag_scrolling: true
            padding: Inset{top: 8 right: 12 bottom: 8 left: 12}

            User := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                draw_bg.color: #x303045
                msg := Label {
                    width: Fill
                    draw_text.color: #xdcdcdc
                    draw_text.text_style.font_size: 13
                }
            }

            Thought := View {
                width: Fill height: Fit
                flow: Down
                margin: Inset{bottom: 4}
                head := Label {
                    width: Fill
                    draw_text.color: #x7a7a7a
                    draw_text.text_style.font_size: 12
                }
                body := Label {
                    width: Fill
                    draw_text.color: #x7a7a7a
                    draw_text.text_style.font_size: 12
                }
            }

            Response := View {
                width: Fill height: Fit
                margin: Inset{bottom: 8}
                msg := Markdown {
                    width: Fill
                    height: Fit
                    body: ""
                    draw_text.color: #xdcdcdc
                    draw_text.text_style.font_size: 13
                }
            }

            ToolDone := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                flow: Down spacing: 4
                draw_bg.color: #x00551a
                head := Label { width: Fill draw_text.color: #xeaeaea draw_text.text_style.font_size: 13 }
                out := Label { width: Fill draw_text.color: #x9a9a9a draw_text.text_style.font_size: 12 }
            }

            ToolRun := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                flow: Down spacing: 4
                draw_bg.color: #x2e2e2e
                head := Label { width: Fill draw_text.color: #xeaeaea draw_text.text_style.font_size: 13 }
                out := Label { width: Fill draw_text.color: #x9a9a9a draw_text.text_style.font_size: 12 }
            }

            ToolFail := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                flow: Down spacing: 4
                draw_bg.color: #x5f1a1a
                head := Label { width: Fill draw_text.color: #xeaeaea draw_text.text_style.font_size: 13 }
                out := Label { width: Fill draw_text.color: #x9a9a9a draw_text.text_style.font_size: 12 }
            }

            Info := View {
                width: Fill height: Fit
                margin: Inset{bottom: 8}
                msg := Label {
                    width: Fill
                    draw_text.color: #x6a6a6a
                    draw_text.text_style.font_size: 12
                }
            }

            Approval := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                flow: Down spacing: 4
                draw_bg.color: #x3a3000
                head := Label { width: Fill draw_text.color: #xeaeaea draw_text.text_style.font_size: 13 }
                buttons := View {
                    width: Fill height: Fit
                    flow: Right spacing: 6
                    approve := Button {
                        text: "Approve"
                        draw_bg.color: #x1a4a2a
                        draw_bg.color_hover: #x2a6a3a
                        draw_bg.color_down: #x0a3a1a
                        draw_text.color: #xeaeaea
                        draw_text.text_style.font_size: 12
                    }
                    deny := Button {
                        text: "Deny"
                        draw_bg.color: #x5f1a1a
                        draw_bg.color_hover: #x7f2a2a
                        draw_bg.color_down: #x4a0a0a
                        draw_text.color: #xeaeaea
                        draw_text.text_style.font_size: 12
                    }
                }
                redirect_row := View {
                    width: Fill height: Fit
                    flow: Right spacing: 6
                    redirect_input := TextInput {
                        width: Fill height: Fit
                        empty_text: "redirect with instructions…"
                        draw_text.color: #xdcdcdc
                        draw_text.text_style.font_size: 12
                        draw_bg.color: #x1a1a20
                        draw_bg.border_size: 1.0
                        draw_bg.border_color: #x4a4a4a
                        is_multiline: false
                        submit_on_enter: false
                    }
                    redirect := Button {
                        text: "Redirect"
                        draw_bg.color: #x4a3a00
                        draw_bg.color_hover: #x6a5000
                        draw_bg.color_down: #x3a2a00
                        draw_text.color: #xeaeaea
                        draw_text.text_style.font_size: 12
                    }
                }
                resolved := Label { width: Fit draw_text.color: #x9a9a9a draw_text.text_style.font_size: 12 }
            }

            SessionEntry := View {
                width: Fill height: Fit
                margin: Inset{bottom: 2}
                btn := Button {
                    width: Fill height: Fit
                    text: ""
                    draw_bg.color: #x1e1e24
                    draw_bg.color_hover: #x2a2a30
                    draw_bg.color_down: #x15151a
                    draw_text.color: #x9a9a9a
                    draw_text.text_style.font_size: 12
                }
            }
        }
    }

    // ModelList wraps a PortalList whose single child is the per-row template.
    let ModelList = #(ModelList::register_widget(vm)) {
        width: Fill
        height: Fill
        list := PortalList {
            width: Fill
            height: Fill
            flow: Down
            auto_tail: false
            drag_scrolling: true
            padding: Inset{top: 4 right: 4 bottom: 4 left: 4}

            row := View {
                width: Fill height: Fit
                margin: Inset{bottom: 2}
                pick := Button {
                    width: Fill height: Fit
                    text: ""
                    draw_bg.color: #x1e1e24
                    draw_bg.color_hover: #x2a2a30
                    draw_bg.color_down: #x15151a
                    draw_text.color: #xcacaca
                    draw_text.text_style.font_size: 12
                }
            }
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                pass.clear_color: #x0f0f12
                window.inner_size: vec2(900, 640)
                window.title: "rho"
                body +: {
                    width: Fill height: Fill
                    flow: Down spacing: 0

                    // ── 1. Title bar ──
                    title_bar := SolidView{
                        width: Fill height: Fit
                        padding: Inset{top: 10, right: 12, bottom: 10, left: 12}
                        flow: Right spacing: 8 align: Align{y: 0.5}
                        draw_bg.color: #x1b1b20
                        title_label := Label{
                            text: "rho"
                            draw_text.color: #xeaeaea
                            draw_text.text_style: theme.font_bold{
                                font_size: 18
                            }
                        }
                        subtitle_label := Label{
                            text: "Rust pair programmer"
                            draw_text.color: #x6a6a6a
                            draw_text.text_style.font_size: 12
                        }
                    }

                    // ── 2. Menu bar — row of buttons ──
                    menu_bar := SolidView{
                        width: Fill height: Fit
                        padding: Inset{top: 4, right: 8, bottom: 4, left: 8}
                        flow: Right spacing: 4 align: Align{y: 0.5}
                        draw_bg.color: #x15151a

                        btn_session := Button{
                            text: "Session"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_model := Button{
                            text: "Model"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_resume := Button{
                            text: "Resume"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_providers := Button{
                            text: "Providers"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_abort := Button{
                            text: "Abort"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_help := Button{
                            text: "Help"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        menu_spacer := View{ width: Fill height: 1 }
                        btn_quit := Button{
                            text: "Quit"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                    }

                    // ── 3. Output scrollback (takes all remaining space) ──
                    chat_scroll := ChatScroll {}

                    // ── 4. Working line (transient, shown during agent turn) ──
                    working := SolidView{
                        visible: false
                        width: Fill height: Fit
                        padding: Inset{top: 4, right: 12, bottom: 4, left: 12}
                        draw_bg.color: #x0f0f12
                        working_text := Label{
                            width: Fill
                            text: "⠹ Working  3.2s  thinking"
                            draw_text.color: #xaaaa00
                            draw_text.text_style.font_size: 12
                        }
                    }

                    // ── 5. Input box (bordered, 2 lines, above footer) ──
                    input_box := RectView{
                        width: Fill height: Fit
                        padding: 0
                        draw_bg.color: #x0000
                        draw_bg.border_size: 1.0
                        draw_bg.border_color: #x4a4a4a
                        input_inner := TextInput{
                            width: Fill height: 48
                            padding: Inset{top: 8, right: 10, bottom: 8, left: 10}
                            empty_text: "Type a message... (Enter to send, Ctrl-J for newline)"
                            draw_text.color: #xdcdcdc
                            draw_text.text_style.font_size: 13
                            draw_bg.color: #x0000
                            draw_bg.border_size: 0.0
                            is_multiline: true
                            submit_on_enter: true
                        }
                    }

                    // ── 6. Footer (2 lines, fixed at bottom) ──
                    footer := SolidView{
                        width: Fill height: Fit
                        padding: Inset{top: 6, right: 12, bottom: 6, left: 12}
                        flow: Down spacing: 2
                        draw_bg.color: #x1b1b20

                        // Line 1: cwd (git-branch)
                        footer_pwd := Label{
                            width: Fill
                            text: "~/dev/rho-coding-agent (main)"
                            draw_text.color: #x6a6a6a
                            draw_text.text_style.font_size: 11
                        }

                        // Line 2: stats (left) ... model (right)
                        footer_stats_row := View{
                            width: Fill height: Fit
                            flow: Right spacing: 8 align: Align{y: 0.5}
                            footer_stats := Label{
                                text: ""
                                draw_text.color: #x6a6a6a
                                draw_text.text_style.font_size: 11
                            }
                            footer_spacer := View{ width: Fill height: 1 }
                            footer_model := Label{
                                text: "claude-sonnet-4-20250514"
                                draw_text.color: #x6a6a6a
                                draw_text.text_style.font_size: 11
                            }
                        }
                    }

                    // ── Model picker modal (overlay; scrollable list) ──
                    model_modal := Modal{
                        content +: {
                            width: 420
                            height: Fit
                            flow: Down

                            SolidView{
                                width: Fill height: Fit
                                padding: Inset{top: 10 right: 10 bottom: 10 left: 10}
                                flow: Down spacing: 8
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Select model"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 13
                                }
                                model_filter_input := TextInput{
                                    width: Fill height: Fit
                                    empty_text: "filter…"
                                    draw_text.color: #xdcdcdc
                                    draw_text.text_style.font_size: 12
                                    draw_bg.color: #x15151a
                                    draw_bg.border_size: 1.0
                                    draw_bg.border_color: #x3a3a40
                                }
                                model_list := ModelList {
                                    width: Fill
                                    height: 340
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── The app, in Rust ───────────────────────────────────────────────────────
// App holds no conversation state itself — it mutates the global CHAT_BLOCKS
// and tells the PortalList to tail + redraw.

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    agent: Option<RhoAgent>,
    #[rust]
    busy: bool,
    #[rust]
    usage: UsageState,
    #[rust]
    models: Vec<String>,
    #[rust]
    filtered_models: Vec<String>,
    #[rust]
    model_filter_text: String,
    #[rust]
    current_model: Option<String>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    working_start: Option<Instant>,
    #[rust]
    working_state: String,
}

#[derive(Default)]
struct UsageState {
    input: u64,
    output: u64,
    cached: u64,
    cost: f64,
    ctx_used: u64,
    ctx_window: u64,
    util: u8,
}

impl App {
    /// Jump the list to the newest block and redraw. With `auto_tail`, the list
    /// then stays pinned to the bottom as long as the user hasn't scrolled up.
    fn tail_and_redraw(&self, cx: &mut Cx) {
        let len = CHAT_BLOCKS.read().unwrap().len();
        let list = self.ui.widget(cx, ids!(chat_scroll)).portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.set_first_id_and_scroll(len.saturating_sub(1), 0.0);
        self.ui.redraw(cx);
    }

    /// Push a block, jump to newest, redraw.
    fn push_block(&self, cx: &mut Cx, block: ChatBlock) {
        CHAT_BLOCKS.write().unwrap().push(block);
        self.tail_and_redraw(cx);
    }

    fn set_busy(&mut self, cx: &mut Cx, busy: bool) {
        self.busy = busy;
        self.ui.widget(cx, ids!(working)).set_visible(cx, busy);
    }

    /// Start an agent turn: show the working line and begin the spinner animation.
    fn start_working(&mut self, cx: &mut Cx) {
        trace!("[busy] -> true (agent/start)");
        self.working_start = Some(Instant::now());
        self.working_state.clear();
        self.set_busy(cx, true);
        self.tick_working(cx);
        self.next_frame = cx.new_next_frame();
    }

    /// End an agent turn: hide the working line and stop animating.
    fn stop_working(&mut self, cx: &mut Cx) {
        trace!("[busy] -> false");
        self.working_start = None;
        self.set_busy(cx, false);
    }

    /// Refresh the working-line label: cycling braille spinner + elapsed + state.
    fn tick_working(&self, cx: &mut Cx) {
        if let Some(start) = self.working_start {
            const SPINNER: [char; 10] = ['⠋','⠙','⠹','⠸','⠼','⠴','⠦','⠧','⠇','⠏'];
            let elapsed = start.elapsed();
            let ch = SPINNER[((elapsed.as_millis() / 80) as usize) % SPINNER.len()];
            let state = if self.working_state.is_empty() {
                "thinking"
            } else {
                &self.working_state
            };
            self.ui
                .label(cx, ids!(working_text))
                .set_text(cx, &format!("{} Working  {:.1}s  {}", ch, elapsed.as_secs_f64(), state));
        }
    }

    /// Write `filtered_models` into the picker modal's global and redraw it.
    fn sync_model_list(&self, cx: &mut Cx) {
        let current = self.current_model.as_deref();
        {
            let mut models = MODELS.write().unwrap();
            models.clear();
            for m in &self.filtered_models {
                models.push((m.clone(), Some(m.as_str()) == current));
            }
        }
        self.ui.redraw(cx);
    }

    /// Recompute the filtered model list from `models` + `model_filter_text`,
    /// then refresh the dropdown.
    fn recompute_filtered_models(&mut self, cx: &mut Cx) {
        let f = self.model_filter_text.to_lowercase();
        self.filtered_models = if f.is_empty() {
            self.models.clone()
        } else {
            self.models
                .iter()
                .filter(|m| m.to_lowercase().contains(&f))
                .cloned()
                .collect()
        };
        self.sync_model_list(cx);
    }

    /// Handle a JSON-RPC response, dispatched by request kind.
    fn handle_response(&mut self, cx: &mut Cx, kind: RequestKind, result: serde_json::Value) {
        match kind {
            RequestKind::GetState => {
                let model = jstr(Some(&result), "model");
                let cwd = jstr(Some(&result), "cwd");
                self.current_model = Some(model.clone());
                self.ui.label(cx, ids!(footer_model)).set_text(cx, &model);
                self.ui.label(cx, ids!(footer_pwd)).set_text(cx, &cwd);
                self.sync_model_list(cx);
            }
            RequestKind::ListModels => {
                self.models = result
                    .get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.get("id").and_then(|x| x.as_str()).map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                self.recompute_filtered_models(cx);
            }
            RequestKind::SetModel => {
                let model = jstr(Some(&result), "model");
                self.current_model = Some(model.clone());
                self.ui.label(cx, ids!(footer_model)).set_text(cx, &model);
                self.sync_model_list(cx);
                self.push_block(cx, ChatBlock::Info(format!("\u{2192} model: {}", model)));
            }
            RequestKind::ListProviders => {
                let mut blocks = CHAT_BLOCKS.write().unwrap();
                if let Some(arr) = result.get("providers").and_then(|x| x.as_array()) {
                    blocks.push(ChatBlock::Info(format!("Providers ({}):", arr.len())));
                    for p in arr {
                        let name = jstr(Some(p), "name");
                        let reachable = jbool(Some(p), "reachable");
                        let active = jbool(Some(p), "active");
                        let ext = jbool(Some(p), "isExternal");
                        let state = if reachable { "\u{2713}" } else { "\u{2717}" };
                        let flags = match (active, ext) {
                            (true, true) => " [active, external]",
                            (true, false) => " [active]",
                            (false, true) => " [external]",
                            (false, false) => "",
                        };
                        blocks.push(ChatBlock::Info(format!("  {} {}{}", name, state, flags)));
                    }
                }
                drop(blocks);
                self.tail_and_redraw(cx);
            }
            RequestKind::ListSessions => {
                let mut blocks = CHAT_BLOCKS.write().unwrap();
                if let Some(arr) = result.get("sessions").and_then(|x| x.as_array()) {
                    if arr.is_empty() {
                        blocks.push(ChatBlock::Info("No previous sessions.".into()));
                    } else {
                        blocks.push(ChatBlock::Info("Click a session to resume:".into()));
                        for s in arr {
                            let path = jstr(Some(s), "path");
                            let entries = ju64(Some(s), "entryCount");
                            blocks.push(ChatBlock::SessionEntry { path, entries });
                        }
                    }
                }
                drop(blocks);
                self.tail_and_redraw(cx);
            }
            RequestKind::ResumeSession => {
                let model = jstr(Some(&result), "model");
                let cwd = jstr(Some(&result), "cwd");
                self.current_model = Some(model.clone());
                self.ui.label(cx, ids!(footer_model)).set_text(cx, &model);
                self.ui.label(cx, ids!(footer_pwd)).set_text(cx, &cwd);
                self.push_block(cx, ChatBlock::Info(format!("\u{21bb} resumed session ({})", cwd)));
            }
            RequestKind::GetSessionStats => {
                let api = result.get("apiUsage");
                self.usage.input = ju64(api, "totalInputTokens");
                self.usage.output = ju64(api, "totalOutputTokens");
                self.usage.cached = ju64(api, "totalCachedTokens");
                self.usage.cost = jf64(api, "totalCost");
                self.usage.ctx_used = ju64(Some(&result), "estimatedUsed");
                self.usage.ctx_window = ju64(Some(&result), "contextWindow");
                self.usage.util = ju64(Some(&result), "utilizationPercent").min(255) as u8;
                self.update_usage(cx);
            }
        }
    }

    /// Resolve an in-scrollback approval prompt: update the block and tell rho.
    fn resolve_approval(
        &mut self,
        cx: &mut Cx,
        index: usize,
        approved: bool,
        message: Option<String>,
    ) {
        let msg = message.filter(|m| !m.is_empty());
        let resolution = match (approved, &msg) {
            (true, _) => ApprovalResolution::Approved,
            (false, Some(m)) => ApprovalResolution::Redirected(m.clone()),
            (false, None) => ApprovalResolution::Denied,
        };
        {
            let mut blocks = CHAT_BLOCKS.write().unwrap();
            if let Some(ChatBlock::Approval { resolution: res, .. }) = blocks.get_mut(index) {
                *res = resolution;
            }
        }
        if let Some(agent) = &mut self.agent {
            let _ = agent.approval_response(approved, msg);
        }
        // The user likely typed the steer into the (now-hidden) redirect_input;
        // return focus to the main input so typing/Enter work immediately.
        self.ui.text_input(cx, ids!(input_inner)).set_key_focus(cx);
        self.tail_and_redraw(cx);
    }

    /// Render the cumulative usage + live context snapshot into the footer.
    fn update_usage(&self, cx: &mut Cx) {
        let u = &self.usage;
        let k = |n: u64| {
            if n >= 1000 {
                format!("{:.1}k", n as f64 / 1000.0)
            } else {
                n.to_string()
            }
        };
        let win = if u.ctx_window >= 1000 {
                format!("{:.0}k", u.ctx_window as f64 / 1000.0)
            } else {
                u.ctx_window.to_string()
            };
        let stats = format!(
            "{} {} R{} ${:.3} {}/{}k(auto)",
            k(u.input), k(u.output), k(u.cached), u.cost, u.util, win
        );
        self.ui.label(cx, ids!(footer_stats)).set_text(cx, &stats);
    }

    /// Map one rho JSON-RPC notification onto CHAT_BLOCKS / UI state.
    fn handle_rho_event(&mut self, cx: &mut Cx, ev: RhoEvent) {
        trace!("[rho-event] {:?}", ev);
        match ev {
            RhoEvent::Ready => {
                self.push_block(cx, ChatBlock::Info("\u{2713} Connected to rho".into()));
                if let Some(agent) = &mut self.agent {
                    let _ = agent.get_state();
                    let _ = agent.list_models();
                    let _ = agent.get_session_stats();
                }
            }
            RhoEvent::AgentStart => {
                self.start_working(cx);
            }
            RhoEvent::MessageDelta { delta } => {
                append_streaming_response(&delta);
                self.tail_and_redraw(cx);
            }
            RhoEvent::ReasoningDelta { delta } => {
                append_streaming_reasoning(&delta);
                self.tail_and_redraw(cx);
            }
            RhoEvent::AgentEnd { reply, duration_ms } => {
                finalize_reasoning(format_secs(duration_ms));
                // If nothing streamed, the final reply is our only text.
                if !reply.is_empty() {
                    let mut blocks = CHAT_BLOCKS.write().unwrap();
                    if !matches!(blocks.last(), Some(ChatBlock::Response(_))) {
                        blocks.push(ChatBlock::Response(reply));
                    }
                }
                self.stop_working(cx);
                if let Some(agent) = &mut self.agent {
                    let _ = agent.get_session_stats();
                }
                self.tail_and_redraw(cx);
            }
            RhoEvent::AgentError { error } => {
                self.push_block(cx, ChatBlock::Info(format!("\u{26a0} {}", error)));
                self.stop_working(cx);
            }
            RhoEvent::StateChange { state } => {
                self.working_state = state.clone();
                self.tick_working(cx);
                // rho-core emits `idle` only on a terminal turn outcome (just
                // before `agent/end`), never mid-turn — retries/compaction return
                // `Thinking`, not `Idle`. So it's a reliable end-of-turn marker:
                // clear busy here so the spinner can never get stuck even if a
                // later `agent/end`/`agent/error` is dropped or reordered.
                if state == "idle" && self.busy {
                    self.stop_working(cx);
                }
            }
            RhoEvent::ToolCall { name, arguments } => {
                CHAT_BLOCKS.write().unwrap().push(ChatBlock::ToolCall {
                    name,
                    args: arguments,
                    status: ToolStatus::Pending,
                    output: None,
                });
                self.tail_and_redraw(cx);
            }
            RhoEvent::ToolResult { name, is_error, output } => {
                let status = if is_error { ToolStatus::Error } else { ToolStatus::Success };
                let output = if output.is_empty() { None } else { Some(output) };
                finalize_tool_call(&name, status, output);
                self.tail_and_redraw(cx);
            }
            RhoEvent::ToolDenied { name } => {
                finalize_tool_call(&name, ToolStatus::Denied, Some("denied by approval gate".into()));
                self.tail_and_redraw(cx);
            }
            RhoEvent::ApprovalRequest { tool, arguments, risk } => {
                CHAT_BLOCKS.write().unwrap().push(ChatBlock::Approval {
                    tool,
                    arguments,
                    risk,
                    resolution: ApprovalResolution::Pending,
                });
                self.tail_and_redraw(cx);
            }
            RhoEvent::Usage {
                input_tokens,
                output_tokens,
                cached_tokens,
                cost,
                context_used,
                context_window,
                utilization,
            } => {
                self.usage.input += input_tokens;
                self.usage.output += output_tokens;
                self.usage.cached += cached_tokens;
                self.usage.cost += cost;
                self.usage.ctx_used = context_used;
                self.usage.ctx_window = context_window;
                self.usage.util = utilization;
                self.update_usage(cx);
            }
            RhoEvent::Response { kind, result } => self.handle_response(cx, kind, result),
            RhoEvent::RequestError { kind, error } => {
                self.push_block(cx, ChatBlock::Info(format!("\u{26a0} {:?} failed: {}", kind, error)));
            }
            RhoEvent::Closed => {
                self.push_block(cx, ChatBlock::Info("rho process exited.".into()));
                self.agent = None;
                self.stop_working(cx);
            }
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // ── Menu button handlers ──
        let ui = self.ui.clone();

        if ui.button(cx, ids!(btn_quit)).clicked(actions) {
            cx.quit();
            return;
        }

        if ui.button(cx, ids!(btn_help)).clicked(actions) {
            self.push_block(cx, ChatBlock::Info("Available commands: Session, Model, Resume, Providers, Abort, Help, Quit. Type a message and press Enter to chat.".into()));
            return;
        }

        if ui.button(cx, ids!(btn_abort)).clicked(actions) {
            let connected = self.agent.is_some();
            if let Some(agent) = &mut self.agent {
                let _ = agent.abort();
            }
            if !connected {
                self.push_block(cx, ChatBlock::Info("nothing to abort.".into()));
            }
            return;
        }

        if ui.button(cx, ids!(btn_session)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                let _ = agent.list_sessions();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return;
        }

        if ui.button(cx, ids!(btn_resume)).clicked(actions) {
            self.push_block(cx, ChatBlock::Info("Resume: pick a session from the Session list (click-to-resume is a later slice).".into()));
            return;
        }

        if ui.button(cx, ids!(btn_providers)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                let _ = agent.list_providers();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return;
        }

        // ── Model picker modal: open, filter, pick ──
        if ui.button(cx, ids!(btn_model)).clicked(actions) {
            self.model_filter_text.clear();
            self.ui.text_input(cx, ids!(model_filter_input)).set_text(cx, "");
            self.filtered_models = self.models.clone();
            self.sync_model_list(cx);
            self.ui.modal(cx, ids!(model_modal)).open(cx);
        }
        if let Some(filter) = self.ui.text_input(cx, ids!(model_filter_input)).changed(actions) {
            self.model_filter_text = filter;
            self.recompute_filtered_models(cx);
        }
        // Pick a model from the modal's list, then close.
        let mut picked: Option<String> = None;
        {
            let list = self.ui.widget(cx, ids!(model_list)).portal_list(cx, ids!(list));
            for (item_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(pick)).clicked(actions) {
                    if let Some((name, _)) = MODELS.read().unwrap().get(item_id) {
                        picked = Some(name.clone());
                    }
                }
            }
        }
        if let Some(model) = picked {
            self.ui.modal(cx, ids!(model_modal)).close(cx);
            if self.current_model.as_deref() != Some(model.as_str()) {
                if let Some(agent) = &mut self.agent {
                    let _ = agent.set_model(&model);
                }
            }
        }

        // ── Inline block actions: approval buttons + click-to-resume sessions ──
        // Collect first, then act, so we don't mutate the portal list mid-iteration.
        let mut approvals: Vec<(usize, bool, Option<String>)> = Vec::new();
        let mut resumes: Vec<String> = Vec::new();
        {
            let list = self.ui.widget(cx, ids!(chat_scroll)).portal_list(cx, ids!(list));
            for (item_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(approve)).clicked(actions) {
                    approvals.push((item_id, true, None));
                }
                if item.button(cx, ids!(deny)).clicked(actions) {
                    approvals.push((item_id, false, None));
                }
                if item.button(cx, ids!(redirect)).clicked(actions) {
                    let msg = item.text_input(cx, ids!(redirect_input)).text();
                    approvals.push((item_id, false, Some(msg)));
                }
                if item.button(cx, ids!(btn)).clicked(actions) {
                    let path = CHAT_BLOCKS.read().unwrap().get(item_id).and_then(|b| match b {
                        ChatBlock::SessionEntry { path, .. } => Some(path.clone()),
                        _ => None,
                    });
                    if let Some(p) = path {
                        resumes.push(p);
                    }
                }
            }
        }
        for (idx, approved, msg) in approvals {
            self.resolve_approval(cx, idx, approved, msg);
        }
        for path in resumes {
            if let Some(agent) = &mut self.agent {
                let _ = agent.resume_session(&path);
            }
        }

        // ── TextInput: Enter to submit ──
        let input = self.ui.text_input(cx, ids!(input_inner));
        if let Some((text, _mods)) = input.returned(actions) {
            if !text.is_empty() {
                input.set_text(cx, "");
                CHAT_BLOCKS.write().unwrap().push(ChatBlock::User(text.clone()));
                let send = match &mut self.agent {
                    Some(agent) => agent.prompt(&text),
                    None => Err("rho agent not connected.".into()),
                };
                if let Err(e) = send {
                    CHAT_BLOCKS
                        .write()
                        .unwrap()
                        .push(ChatBlock::Info(format!("\u{26a0} {}", e)));
                }
                self.tail_and_redraw(cx);
            }
            // Keep typing: submitting (or pressing Enter on an empty box) can drop key
            // focus, so re-assert it on the input every time.
            input.set_key_focus(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        // On startup, spawn the rho agent. The reader threads will wake us via
        // SignalToUI when rho emits its `ready` notification.
        if let Event::Startup = event {
            self.push_block(cx, ChatBlock::Info("Starting rho\u{2026}".into()));
            match RhoAgent::spawn() {
                Ok(agent) => self.agent = Some(agent),
                Err(e) => self.push_block(
                    cx,
                    ChatBlock::Info(format!(
                        "\u{26a0} Could not start rho: {}\nSet RHO_PATH to the rho binary, or put rho on PATH.",
                        e
                    )),
                ),
            }
        }

        // Drain subprocess output the reader threads queued since the last Signal.
        if let Event::Signal = event {
            let events = self.agent.as_mut().map(|a| a.drain()).unwrap_or_default();
            for ev in events {
                self.handle_rho_event(cx, ev);
            }
        }

        // Animate the working-line spinner while an agent turn is in flight.
        if self.busy && self.next_frame.is_event(event).is_some() {
            self.tick_working(cx);
            self.next_frame = cx.new_next_frame();
        }
    }
}
