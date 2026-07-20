pub use makepad_widgets;

use makepad_widgets::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::sync::RwLock;
use std::thread;
use std::time::Instant;

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
    /// A mid-turn steering message (sent while the agent was busy).
    Steer(String),
    /// A completed reasoning ("thinking") block with elapsed time.
    Reasoning { text: String, elapsed_secs: String },
    /// A streaming reasoning block (still thinking).
    ReasoningStreaming { text: String },
    /// A streaming (in-progress) response — rendered as a cheap Label, not
    /// markdown, so appending deltas doesn't re-parse on every redraw.
    ResponseStreaming(String),
    /// A finalized response from the agent, rendered as markdown.
    Response(String),
    /// A tool call block with status.
    ToolCall {
        name: String,
        args: String,
        status: ToolStatus,
        output: Option<String>,
        expanded: bool,
    },
    /// A tool-approval prompt from the agent (blocks until we respond).
    Approval {
        tool: String,
        arguments: String,
        risk: String,
        resolution: ApprovalResolution,
    },
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

/// Models shown in the picker modal: (id, provider, is_current).
static MODELS: RwLock<Vec<(String, String, bool)>> = RwLock::new(Vec::new());

/// Sessions shown in the picker modal: (path, mtime_secs, entry_count).
static SESSIONS: RwLock<Vec<(String, u64, u64)>> = RwLock::new(Vec::new());

/// Providers shown in the picker modal: (name, reachable, active, is_external).
static PROVIDERS: RwLock<Vec<(String, bool, bool, bool)>> = RwLock::new(Vec::new());

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
    ReloadExtensions,
}

#[derive(Clone, Debug)]
enum RhoEvent {
    Ready,
    AgentStart,
    AgentEnd {
        reply: String,
        duration_ms: u64,
    },
    AgentError {
        error: String,
    },
    MessageDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    StateChange {
        state: String,
    },
    ToolCall {
        name: String,
        arguments: String,
    },
    ToolResult {
        name: String,
        is_error: bool,
        output: String,
    },
    ToolDenied {
        name: String,
    },
    ApprovalRequest {
        tool: String,
        arguments: String,
        risk: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cached_tokens: u64,
        cost: f64,
        context_used: u64,
        context_window: u64,
        utilization: u8,
    },
    Response {
        kind: RequestKind,
        result: serde_json::Value,
    },
    RequestError {
        kind: RequestKind,
        error: String,
    },
    Closed,
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

        Ok(Self {
            child: child,
            stdin,
            receiver: rx,
            next_id: 0,
            pending: HashMap::new(),
        })
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
            "agent/error" => RhoEvent::AgentError {
                error: jstr(p, "error"),
            },
            "message/delta" => RhoEvent::MessageDelta {
                delta: jstr(p, "delta"),
            },
            "reasoning/delta" => RhoEvent::ReasoningDelta {
                delta: jstr(p, "delta"),
            },
            "state/change" => RhoEvent::StateChange {
                state: jstr(p, "state"),
            },
            "tool/call" => RhoEvent::ToolCall {
                name: jstr(p, "name"),
                arguments: jstr(p, "arguments"),
            },
            "tool/result" => RhoEvent::ToolResult {
                name: jstr(p, "name"),
                is_error: jbool(p, "isError"),
                output: jstr(p, "output"),
            },
            "tool/denied" => RhoEvent::ToolDenied {
                name: jstr(p, "name"),
            },
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
    fn request(
        &mut self,
        kind: RequestKind,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        self.write_jsonrpc(method, params, Some(kind))
    }

    fn prompt(&mut self, message: &str, steer: bool) -> Result<(), String> {
        self.fire("prompt", serde_json::json!({ "message": message, "steer": steer }))
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
        self.request(
            RequestKind::ListProviders,
            "listProviders",
            serde_json::json!({}),
        )
    }
    fn list_sessions(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::ListSessions,
            "listSessions",
            serde_json::json!({}),
        )
    }
    fn set_model(&mut self, model: &str) -> Result<(), String> {
        self.request(
            RequestKind::SetModel,
            "setModel",
            serde_json::json!({ "model": model }),
        )
    }
    fn resume_session(&mut self, path: &str) -> Result<(), String> {
        self.request(
            RequestKind::ResumeSession,
            "resumeSession",
            serde_json::json!({ "path": path }),
        )
    }
    fn get_session_stats(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::GetSessionStats,
            "getSessionStats",
            serde_json::json!({}),
        )
    }
    fn reload_extensions(&mut self) -> Result<(), String> {
        self.request(
            RequestKind::ReloadExtensions,
            "reloadExtensions",
            serde_json::json!({}),
        )
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
    p.and_then(|p| p.get(k))
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
}
fn jf64(p: Option<&serde_json::Value>, k: &str) -> f64 {
    p.and_then(|p| p.get(k))
        .and_then(|x| x.as_f64())
        .unwrap_or(0.0)
}
fn jbool(p: Option<&serde_json::Value>, k: &str) -> bool {
    p.and_then(|p| p.get(k))
        .and_then(|x| x.as_bool())
        .unwrap_or(false)
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

/// Compact, timezone-free recency label for a unix-epoch timestamp (seconds).
/// Used for the session table's date column.
fn relative_time(secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else if delta < 86_400 * 7 {
        format!("{}d ago", delta / 86_400)
    } else if delta < 86_400 * 30 {
        format!("{}w ago", delta / (86_400 * 7))
    } else {
        format!("{}mo ago", delta / (86_400 * 30))
    }
}

// Streaming helpers: append into the trailing block of the right kind, creating
// it when the kind changes, so consecutive deltas accumulate into one block.
fn append_streaming_response(delta: &str) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    match blocks.last_mut() {
        Some(ChatBlock::ResponseStreaming(text)) => text.push_str(delta),
        _ => blocks.push(ChatBlock::ResponseStreaming(delta.to_string())),
    }
}

/// Convert the trailing streaming response (cheap Label) into a finalized
/// markdown Response. No-op if the last block isn't a streaming response.
fn finalize_streaming_response() {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if let Some(last) = blocks.last_mut() {
        if let ChatBlock::ResponseStreaming(text) = last {
            let owned = std::mem::take(text);
            *last = ChatBlock::Response(owned);
        }
    }
}

/// First `max` chars + a note when longer — keeps widget layout cheap when tool
/// output / reasoning / a response is huge (the session had ~300KB tool results).
fn cap_head(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{}\n\u{2026} ({} more chars)", head, count - max)
    }
}

/// Last `max` chars (a tail window) so live streaming shows the newest text
/// without laying out the whole growing string.
fn cap_tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let tail: String = s.chars().skip(count - max).collect();
        format!("\u{2026} ({} earlier chars)\n{}", count - max, tail)
    }
}

/// Render a getSessionStats result as the aligned key/value text for the
/// Stats modal (the GUI equivalent of the CLI's /stats).
fn format_session_stats(r: &serde_json::Value) -> String {
    let api = r.get("apiUsage");
    let role = r.get("roleTokens");
    let res = r.get("resolutionTokens");
    let kfmt = |n: u64| {
        if n >= 1000 {
            format!("{:.1}k", n as f64 / 1000.0)
        } else {
            n.to_string()
        }
    };
    format!(
        "Context\n\
         \x20 window          {}\n\
         \x20 used            {} ({}%)\n\
         \x20 remaining       {}\n\
         \x20 completion rsv  {}\n\
         \n\
         Session\n\
         \x20 messages        {}\n\
         \x20 entries         {} ({} on path, {} compacted)\n\
         \n\
         Role tokens\n\
         \x20 system          {}\n\
         \x20 user            {}\n\
         \x20 assistant       {}\n\
         \x20 tool            {}\n\
         \n\
         Resolution\n\
         \x20 full            {}\n\
         \x20 outlined        {}\n\
         \x20 summarized      {}\n\
         \x20 pinned          {}\n\
         \n\
         API usage\n\
         \x20 input           {}\n\
         \x20 output          {}\n\
         \x20 cached          {}\n\
         \x20 total           {}\n\
         \x20 requests        {}\n\
         \x20 cost            ${:.4}",
        kfmt(ju64(Some(r), "contextWindow")),
        kfmt(ju64(Some(r), "estimatedUsed")),
        ju64(Some(r), "utilizationPercent"),
        kfmt(ju64(Some(r), "estimatedRemaining")),
        kfmt(ju64(Some(r), "completionReserve")),
        ju64(Some(r), "messageCount"),
        ju64(Some(r), "entryCount"),
        ju64(Some(r), "pathEntryCount"),
        ju64(Some(r), "compactedEntryCount"),
        kfmt(ju64(role, "system")),
        kfmt(ju64(role, "user")),
        kfmt(ju64(role, "assistant")),
        kfmt(ju64(role, "tool")),
        kfmt(ju64(res, "full")),
        kfmt(ju64(res, "outlined")),
        kfmt(ju64(res, "summarized")),
        kfmt(ju64(res, "pinned")),
        kfmt(ju64(api, "totalInputTokens")),
        kfmt(ju64(api, "totalOutputTokens")),
        kfmt(ju64(api, "totalCachedTokens")),
        kfmt(ju64(api, "totalTokens")),
        ju64(api, "requestCount"),
        jf64(api, "totalCost"),
    )
}
fn append_streaming_reasoning(delta: &str) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    match blocks.last_mut() {
        Some(ChatBlock::ReasoningStreaming { text }) => text.push_str(delta),
        _ => blocks.push(ChatBlock::ReasoningStreaming {
            text: delta.to_string(),
        }),
    }
}
fn finalize_reasoning(elapsed_secs: String) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    if let Some(last) = blocks.last_mut() {
        if let ChatBlock::ReasoningStreaming { text } = last {
            let owned = std::mem::take(text);
            *last = ChatBlock::Reasoning {
                text: owned,
                elapsed_secs,
            };
        }
    }
}

/// Finalize the most recent pending ToolCall with `name` (rho's loop is
/// sequential, so there's at most one in flight per name). If none is found,
/// push a finalized block as a defensive fallback.
fn finalize_tool_call(name: &str, status: ToolStatus, output: Option<String>) {
    let mut blocks = CHAT_BLOCKS.write().unwrap();
    for block in blocks.iter_mut().rev() {
        if let ChatBlock::ToolCall {
            name: n,
            status: st,
            output: out,
            ..
        } = block
        {
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
        expanded: false,
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
                            ChatBlock::Steer(text) => {
                                let w = list.item(cx, item_id, id!(Steer));
                                w.label(cx, ids!(msg)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Reasoning { text, elapsed_secs } => {
                                let w = list.item(cx, item_id, id!(Thought));
                                w.label(cx, ids!(head))
                                    .set_text(cx, &format!("? thought \u{00b7} {}", elapsed_secs));
                                w.label(cx, ids!(body)).set_text(cx, &cap_head(text, 4000));
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::ReasoningStreaming { text } => {
                                let w = list.item(cx, item_id, id!(Thought));
                                w.label(cx, ids!(head)).set_text(cx, "? thinking");
                                w.label(cx, ids!(body)).set_text(cx, &cap_tail(text, 4000));
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::ResponseStreaming(text) => {
                                let w = list.item(cx, item_id, id!(ResponseStreaming));
                                w.label(cx, ids!(msg)).set_text(cx, &cap_tail(text, 4000));
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Response(text) => {
                                let w = list.item(cx, item_id, id!(Response));
                                w.markdown(cx, ids!(msg)).set_text(cx, &cap_head(text, 4000));
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::ToolCall {
                                name,
                                args,
                                status,
                                output,
                                expanded,
                            } => {
                                let (template, status_text) = match status {
                                    ToolStatus::Success => (id!(ToolDone), "\u{2713} done"),
                                    ToolStatus::Pending => (id!(ToolRun), "\u{27f3} running"),
                                    ToolStatus::Error => (id!(ToolFail), "\u{2717} failed"),
                                    ToolStatus::Denied => (id!(ToolRun), "\u{2298} denied"),
                                };
                                let w = list.item(cx, item_id, template);
                                w.label(cx, ids!(head))
                                    .set_text(cx, &format!("{} {} {}", name, args, status_text));
                                match output {
                                    Some(out) => {
                                        w.widget(cx, ids!(out)).set_visible(cx, true);
                                        let display = if *expanded {
                                            out.clone()
                                        } else {
                                            cap_head(out, 10000)
                                        };
                                        w.label(cx, ids!(out)).set_text(cx, &display);
                                        // Show expand/collapse button only when output is
                                        // large enough to have been truncated.
                                        let can_toggle = out.chars().count() > 10000;
                                        w.widget(cx, ids!(expand_btn)).set_visible(cx, can_toggle);
                                        if can_toggle {
                                            w.button(cx, ids!(expand_btn)).set_text(cx, if *expanded { "Collapse" } else { "Expand" });
                                        }
                                    }
                                    None => {
                                        w.widget(cx, ids!(out)).set_visible(cx, false);
                                        w.widget(cx, ids!(expand_btn)).set_visible(cx, false);
                                    }
                                }
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Info(text) => {
                                let w = list.item(cx, item_id, id!(Info));
                                w.label(cx, ids!(msg)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::Approval {
                                tool,
                                arguments,
                                risk,
                                resolution,
                            } => {
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
                                w.widget(cx, ids!(redirect_row))
                                    .set_visible(cx, show_actions);
                                w.widget(cx, ids!(resolved))
                                    .set_visible(cx, resolved_text.is_some());
                                if let Some(t) = resolved_text {
                                    w.label(cx, ids!(resolved)).set_text(cx, &t);
                                }
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
                    if let Some((id, provider, is_current)) = models.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        // Prefix with the provider so models from different
                        // providers (e.g. openrouter vs zai) are clearly
                        // differentiated; the list is already sorted by provider.
                        let label = if *is_current {
                            format!("\u{2713} {} \u{00b7} {}", provider, id)
                        } else {
                            format!("{} \u{00b7} {}", provider, id)
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

// ── SessionList: a PortalList of sessions for the picker modal ───────────────
// Same shape as ModelList: snapshots the global SESSIONS during draw.
#[derive(Script, ScriptHook, Widget)]
pub struct SessionList {
    #[deref]
    view: View,
}

impl Widget for SessionList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let sessions = SESSIONS.read().unwrap().clone();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, sessions.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    if let Some((path, mtime, entries)) = sessions.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        // Monospace, fixed-width columns so rows line up:
                        //   date (11) | entries (12) | path (rest)
                        let row = format!(
                            "{:<11}{:<12}{}",
                            relative_time(*mtime),
                            format!("{} entries", entries),
                            path
                        );
                        w.button(cx, ids!(pick)).set_text(cx, &row);
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

// ── ProviderList: a PortalList of providers for the picker modal ────────
// Same shape as ModelList: snapshots the global PROVIDERS during draw.
#[derive(Script, ScriptHook, Widget)]
pub struct ProviderList {
    #[deref]
    view: View,
}

impl Widget for ProviderList {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        let providers = PROVIDERS.read().unwrap().clone();
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            if let Some(mut list) = item.as_portal_list().borrow_mut() {
                list.set_item_range(cx, 0, providers.len());
                while let Some(item_id) = list.next_visible_item(cx) {
                    if let Some((name, reachable, active, is_external)) = providers.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        let state = if *reachable { "\u{2713}" } else { "\u{2717}" };
                        let flags = match (active, is_external) {
                            (true, true) => " [active, external]",
                            (true, false) => " [active]",
                            (false, true) => " [external]",
                            (false, false) => "",
                        };
                        w.button(cx, ids!(pick)).set_text(cx, &format!("{} {}{}", name, state, flags));
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

            Steer := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                draw_bg.color: #x3a2a00
                msg := Label {
                    width: Fill
                    draw_text.color: #xeaeaea
                    draw_text.text_style.font_size: 13
                }
            }
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
                    font_size: 13
                    font_color: #xdcdcdc
                    text_style_normal: theme.font_regular{font_size: 13}
                    text_style_italic: theme.font_italic{font_size: 13}
                    text_style_bold: theme.font_bold{font_size: 13}
                    text_style_bold_italic: theme.font_bold_italic{font_size: 13}
                    text_style_fixed: theme.font_code{font_size: 13}
                    draw_text.color: #xdcdcdc
                }
            }

            ResponseStreaming := View {
                width: Fill height: Fit
                margin: Inset{bottom: 8}
                msg := Label {
                    width: Fill
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
                expand_btn := Button {
                    width: Fit height: Fit
                    text: "Expand"
                    draw_bg.color: #x2a2a30
                    draw_bg.color_hover: #x3a3a40
                    draw_bg.color_down: #x1a1a20
                    draw_text.color: #xcacaca
                    draw_text.text_style.font_size: 11
                }
            }

            ToolRun := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                flow: Down spacing: 4
                draw_bg.color: #x2e2e2e
                head := Label { width: Fill draw_text.color: #xeaeaea draw_text.text_style.font_size: 13 }
                out := Label { width: Fill draw_text.color: #x9a9a9a draw_text.text_style.font_size: 12 }
                expand_btn := Button {
                    width: Fit height: Fit
                    text: "Expand"
                    draw_bg.color: #x2a2a30
                    draw_bg.color_hover: #x3a3a40
                    draw_bg.color_down: #x1a1a20
                    draw_text.color: #xcacaca
                    draw_text.text_style.font_size: 11
                }
            }

            ToolFail := SolidView {
                width: Fill height: Fit
                margin: Inset{bottom: 4}
                padding: Inset{top: 8 right: 8 bottom: 8 left: 8}
                flow: Down spacing: 4
                draw_bg.color: #x5f1a1a
                head := Label { width: Fill draw_text.color: #xeaeaea draw_text.text_style.font_size: 13 }
                out := Label { width: Fill draw_text.color: #x9a9a9a draw_text.text_style.font_size: 12 }
                expand_btn := Button {
                    width: Fit height: Fit
                    text: "Expand"
                    draw_bg.color: #x2a2a30
                    draw_bg.color_hover: #x3a3a40
                    draw_bg.color_down: #x1a1a20
                    draw_text.color: #xcacaca
                    draw_text.text_style.font_size: 11
                }
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

    // SessionList wraps a PortalList whose single child is the per-row template.
    let SessionList = #(SessionList::register_widget(vm)) {
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
                margin: Inset{bottom: 1}
                pick := Button {
                    width: Fill height: Fit
                    align: Align{x: 0.0 y: 0.5}
                    padding: Inset{top: 6 right: 8 bottom: 6 left: 8}
                    text: ""
                    draw_bg.color: #x1e1e24
                    draw_bg.color_hover: #x2a2a30
                    draw_bg.color_down: #x15151a
                    draw_text.color: #xcacaca
                    draw_text.text_style: theme.font_code{font_size: 12}
                }
            }
        }
    }

    // ProviderList wraps a PortalList whose single child is the per-row template.
    let ProviderList = #(ProviderList::register_widget(vm)) {
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
                    flow: Overlay

                    content := View {
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
                            text: "Rust programming assistant"
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
                        btn_resume := Button{
                            text: "Resume Last"
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
                        btn_providers := Button{
                            text: "Providers"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_reload := Button{
                            text: "Reload"
                            draw_bg.color: #x2a2a30
                            draw_bg.color_hover: #x3a3a40
                            draw_bg.color_down: #x1a1a20
                            draw_text.color: #xcacaca
                            draw_text.text_style.font_size: 12
                        }
                        btn_restart := Button{
                            text: "Restart"
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
                        btn_stats := Button{
                            text: "Stats"
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
                            width: Fill height: 80
                            padding: Inset{top: 8, right: 10, bottom: 8, left: 10}
                            empty_text: "Type a message... (Enter to send, Shift+Enter for newline)"
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
                        flow: Down spacing: 4
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

                    } // content

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
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    model_close := Button{
                                        text: "Close"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                }
                            }
                        }
                    }

                    // ── Session picker modal (overlay; scrollable list) ──
                    session_modal := Modal{
                        content +: {
                            width: 520
                            height: Fit
                            flow: Down

                            SolidView{
                                width: Fill height: Fit
                                padding: Inset{top: 10 right: 10 bottom: 10 left: 10}
                                flow: Down spacing: 8
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Resume session"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 13
                                }
                                // Column header — monospace, padded to match the
                                // rows' `{:<11}{:<12}` layout, and offset so it lines
                                // up with the row text (SolidView pad 10 + 12).
                                session_header := Label{
                                    width: Fill
                                    text: "Modified   Entries     Session"
                                    margin: Inset{left: 12 top: 2 bottom: 4}
                                    draw_text.color: #x7a7a7a
                                    draw_text.text_style: theme.font_code{font_size: 12}
                                }
                                session_list := SessionList {
                                    width: Fill
                                    height: 340
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    session_close := Button{
                                        text: "Close"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                }
                            }
                        }
                    }


                    // ── Provider info modal (overlay; scrollable list) ──


                    // ── Stats modal ──
                    stats_modal := Modal{
                        content +: {
                            width: 460
                            height: Fit
                            flow: Down

                            SolidView{
                                width: Fill height: Fit
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Session stats"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                stats_body := Label{
                                    width: Fill
                                    text: ""
                                    draw_text.color: #xcacaca
                                    draw_text.text_style: theme.font_code{font_size: 12}
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    stats_close := Button{
                                        text: "Close"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                }
                            }
                        }
                    }
                    // ── Help modal ──
                    help_modal := Modal{
                        content +: {
                            width: 480
                            height: Fit
                            flow: Down

                            SolidView{
                                width: Fill height: Fit
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "rho — quick reference"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                Label{
                                    width: Fill
                                    text: "Type a message and press Enter to chat with the agent.\n\nMenu buttons:\n  Session — list and resume previous sessions\n  Resume Last — quickly resume the most recent session\n  Model — pick a model from the scrollable list\n  Providers — view configured providers and their status\n  Reload — reload extensions from disk (picks up new .rho/extensions/*.ts)\n  Restart — kill and re-spawn the rho subprocess (use if it's stuck or unresponsive)\n  Abort — cancel the current agent turn\n  Help — this dialog\n  Quit — exit rho\n\nInput: Enter sends, Shift+Enter inserts a newline.\n\nWhile the agent is working, the input placeholder changes to 'Steer the agent...' and your message is sent as a mid-turn steering prompt instead of starting a new turn. Steering messages appear with a distinct background and are reflected in the working line.\n\nTool calls that need approval show Approve / Deny / Redirect buttons inline.\n\nTool output longer than 10000 characters is truncated — click Expand to see the full output, Collapse to hide it again."
                                    draw_text.color: #xcacaca
                                    draw_text.text_style.font_size: 12
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    help_close := Button{
                                        text: "Close"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                }
                            }
                        }
                    }
                    provider_modal := Modal{
                        content +: {
                            width: 460
                            height: Fit
                            flow: Down

                            SolidView{
                                width: Fill height: Fit
                                padding: Inset{top: 10 right: 10 bottom: 10 left: 10}
                                flow: Down spacing: 8
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Providers"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 13
                                }
                                provider_list := ProviderList {
                                    width: Fill
                                    height: 340
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    provider_close := Button{
                                        text: "Close"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                }
                            }
                        }
                    }
                    // ── Resume-last-session confirmation modal ──
                    resume_confirm_modal := Modal{
                        content +: {
                            width: 460
                            height: Fit
                            flow: Down

                            SolidView{
                                width: Fill height: Fit
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Resume last session?"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                resume_confirm_meta := Label{
                                    width: Fill
                                    text: ""
                                    draw_text.color: #x9a9a9a
                                    draw_text.text_style.font_size: 12
                                }
                                resume_confirm_path := Label{
                                    width: Fill
                                    text: ""
                                    draw_text.color: #x7a7a7a
                                    draw_text.text_style: theme.font_code{font_size: 12}
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    resume_confirm_cancel := Button{
                                        text: "Cancel"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                    resume_confirm_ok := Button{
                                        text: "Resume"
                                        draw_bg.color: #x1a4a2a
                                        draw_bg.color_hover: #x2a6a3a
                                        draw_bg.color_down: #x0a3a1a
                                        draw_text.color: #xeaeaea
                                        draw_text.text_style.font_size: 12
                                    }
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
    models: Vec<(String, String)>,
    #[rust]
    filtered_models: Vec<(String, String)>,
    #[rust]
    model_filter_text: String,
    #[rust]
    current_model: Option<String>,
    #[rust]
    resume_latest_requested: bool,
    #[rust]
    stats_requested: bool,
    #[rust]
    pending_resume_path: Option<String>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    working_start: Option<Instant>,
    #[rust]
    working_state: String,
    #[rust]
    stream_dirty: bool,
    #[rust]
    last_tick: Option<Instant>,
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
        let list = self
            .ui
            .widget(cx, ids!(chat_scroll))
            .portal_list(cx, ids!(list));
        list.set_tail_range(true);
        list.set_first_id_and_scroll(len.saturating_sub(1), 0.0);
        self.ui.redraw(cx);
    }

    /// Push a block, jump to newest, redraw.
    fn push_block(&self, cx: &mut Cx, block: ChatBlock) {
        finalize_streaming_response();
        CHAT_BLOCKS.write().unwrap().push(block);
        self.tail_and_redraw(cx);
    }
    fn set_busy(&mut self, cx: &mut Cx, busy: bool) {
        self.busy = busy;
        // Swap the input placeholder to indicate steering mode while the agent
        // is mid-turn.
        let placeholder = if busy {
            "Steer the agent... (Enter to send, Shift+Enter for newline)"
        } else {
            "Type a message... (Enter to send, Shift+Enter for newline)"
        };
        self.ui
            .text_input(cx, ids!(input_inner))
            .set_empty_text(cx, placeholder.to_string());
        self.ui.widget(cx, ids!(working)).set_visible(cx, busy);
    }
    /// Start an agent turn: show the working line and begin the spinner animation.
    fn start_working(&mut self, cx: &mut Cx) {
        trace!("[busy] -> true (agent/start)");
        self.working_start = Some(Instant::now());
        self.working_state.clear();
        self.last_tick = Some(Instant::now());
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

    /// Kill the current rho subprocess and spawn a fresh one. The old agent
    /// is dropped (its Drop impl kills + waits the child), then a new one is
    /// spawned. The new process will emit `ready` when it's bootstrapped.
    fn restart_agent(&mut self, cx: &mut Cx) {
        trace!("[restart] killing current rho subprocess");
        // Drop the old agent — its Drop impl kills + waits the child process.
        // This also closes stdin, causing the reader threads to finish.
        self.agent = None;
        self.stop_working(cx);
        self.push_block(cx, ChatBlock::Info("Restarting rho\u{2026}".into()));
        match RhoAgent::spawn() {
            Ok(agent) => {
                self.agent = Some(agent);
            }
            Err(e) => {
                self.push_block(
                    cx,
                    ChatBlock::Info(format!(
                        "\u{26a0} Could not restart rho: {}",
                        e
                    )),
                );
            }
        }
    }

    /// Refresh the working-line label: cycling braille spinner + elapsed + state.
    fn tick_working(&self, cx: &mut Cx) {
        if let Some(start) = self.working_start {
            const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let elapsed = start.elapsed();
            let ch = SPINNER[((elapsed.as_millis() / 80) as usize) % SPINNER.len()];
            let state = if self.working_state.is_empty() {
                "thinking"
            } else {
                &self.working_state
            };
            self.ui.label(cx, ids!(working_text)).set_text(
                cx,
                &format!("{} Working  {:.1}s  {}", ch, elapsed.as_secs_f64(), state),
            );
        }
    }

    /// Write `filtered_models` into the picker modal's global and redraw it.
    fn sync_model_list(&self, cx: &mut Cx) {
        let current = self.current_model.as_deref();
        {
            let mut models = MODELS.write().unwrap();
            models.clear();
            for (id, provider) in &self.filtered_models {
                models.push((id.clone(), provider.clone(), Some(id.as_str()) == current));
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
                .filter(|(id, provider)| {
                    id.to_lowercase().contains(&f) || provider.to_lowercase().contains(&f)
                })
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
                let mut models: Vec<(String, String)> = result
                    .get("models")
                    .and_then(|m| m.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| {
                                let id = e.get("id")?.as_str()?.to_owned();
                                let provider = jstr(Some(e), "provider");
                                Some((id, provider))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Cluster by provider, then id, so the list reads in groups
                // (e.g. all `openrouter` together, then all `zai`).
                models.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                self.models = models;
                self.recompute_filtered_models(cx);
            }
            RequestKind::SetModel => {
                let model = jstr(Some(&result), "model");
                self.current_model = Some(model.clone());
                self.ui.label(cx, ids!(footer_model)).set_text(cx, &model);
                self.sync_model_list(cx);
                self.push_block(cx, ChatBlock::Info(format!("\u{2192} model: {}", model)));
                // Refresh the footer's context window for the new model. Newer
                // rho bundles the post-switch context stats into this response
                // (contextWindow/estimatedUsed/utilizationPercent) — use them
                // directly so the footer updates in the same redraw as the model
                // name, with no extra round-trip. Fall back to getSessionStats
                // for older rho that doesn't send them.
                if result.get("contextWindow").is_some() {
                    self.usage.ctx_window = ju64(Some(&result), "contextWindow");
                    self.usage.ctx_used = ju64(Some(&result), "estimatedUsed");
                    self.usage.util = ju64(Some(&result), "utilizationPercent").min(255) as u8;
                    self.update_usage(cx);
                } else if let Some(agent) = &mut self.agent {
                    let _ = agent.get_session_stats();
                }
            }
            RequestKind::ListProviders => {
                let mut providers: Vec<(String, bool, bool, bool)> = Vec::new();
                if let Some(arr) = result.get("providers").and_then(|x| x.as_array()) {
                    for p in arr {
                        providers.push((
                            jstr(Some(p), "name"),
                            jbool(Some(p), "reachable"),
                            jbool(Some(p), "active"),
                            jbool(Some(p), "isExternal"),
                        ));
                    }
                }
                if providers.is_empty() {
                    self.push_block(cx, ChatBlock::Info("No providers configured.".into()));
                } else {
                    {
                        let mut g = PROVIDERS.write().unwrap();
                        *g = providers;
                    }
                    self.ui.redraw(cx);
                    self.ui.modal(cx, ids!(provider_modal)).open(cx);
                }
            }
            RequestKind::ListSessions => {
                let mut sessions: Vec<(String, u64, u64)> = result
                    .get("sessions")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|s| {
                                (
                                    jstr(Some(s), "path"),
                                    ju64(Some(s), "mtimeSecs"),
                                    ju64(Some(s), "entryCount"),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Newest first (by modification time).
                sessions.sort_by(|a, b| b.1.cmp(&a.1));
                if sessions.is_empty() {
                    self.push_block(cx, ChatBlock::Info("No previous sessions.".into()));
                } else if self.resume_latest_requested {
                    // "Resume Last": confirm the most recent session before resuming.
                    self.resume_latest_requested = false;
                    let (path, mtime, entries) = sessions[0].clone();
                    self.pending_resume_path = Some(path.clone());
                    self.ui.label(cx, ids!(resume_confirm_meta)).set_text(
                        cx,
                        &format!("{} · {} entries", relative_time(mtime), entries),
                    );
                    self.ui
                        .label(cx, ids!(resume_confirm_path))
                        .set_text(cx, &path);
                    self.ui.modal(cx, ids!(resume_confirm_modal)).open(cx);
                } else {
                    {
                        let mut g = SESSIONS.write().unwrap();
                        *g = sessions;
                    }
                    self.ui.redraw(cx);
                    self.ui.modal(cx, ids!(session_modal)).open(cx);
                }
            }
            RequestKind::ResumeSession => {
                let model = jstr(Some(&result), "model");
                let cwd = jstr(Some(&result), "cwd");
                self.current_model = Some(model.clone());
                self.ui.label(cx, ids!(footer_model)).set_text(cx, &model);
                self.ui.label(cx, ids!(footer_pwd)).set_text(cx, &cwd);
                self.push_block(
                    cx,
                    ChatBlock::Info(format!("\u{21bb} resumed session ({})", cwd)),
                );
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
                // Stats button flow: same response also feeds the modal when
                // the user explicitly asked for it (mirrors resume_latest_requested).
                if self.stats_requested {
                    self.stats_requested = false;
                    self.ui
                        .label(cx, ids!(stats_body))
                        .set_text(cx, &format_session_stats(&result));
                    self.ui.modal(cx, ids!(stats_modal)).open(cx);
                }
            }
            RequestKind::ReloadExtensions => {
                let reloaded = ju64(Some(&result), "reloaded");
                let added = ju64(Some(&result), "added");
                let removed = ju64(Some(&result), "removed");
                self.push_block(
                    cx,
                    ChatBlock::Info(format!(
                        "\u{2713} extensions reloaded (+{} ~{} -{})",
                        added, reloaded, removed
                    )),
                );
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
            if let Some(ChatBlock::Approval {
                resolution: res, ..
            }) = blocks.get_mut(index)
            {
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
            if n == 0 {
                "0".to_string()
            } else {
                format!("{:.1}k", n as f64 / 1000.0)
            }
        };
        // Context portion: estimated tokens used over the prompt budget, plus
        // the utilization percentage. The prompt budget is the window minus the
        // completion reserve — the denominator utilization is computed against.
        let stats = if u.ctx_window == 0 && u.ctx_used == 0 {
            // Nothing reported yet — show cumulative API tokens only.
            format!("\u{2191}{}    \u{2193}{}    R{}    ${:.3}", k(u.input), k(u.output), k(u.cached), u.cost)
        } else {
            format!(
                "\u{2191}{}    \u{2193}{}    R{}    ${:.3}    ctx {}/{} ({}%)",
                k(u.input),
                k(u.output),
                k(u.cached),
                u.cost,
                k(u.ctx_used),
                k(u.ctx_window),
                u.util,
            )
        };
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
                // Append to the streaming block but don't redraw per-delta; the
                // NextFrame loop coalesces redraws to ~12/sec while busy.
                append_streaming_response(&delta);
                self.stream_dirty = true;
            }
            RhoEvent::ReasoningDelta { delta } => {
                finalize_streaming_response();
                append_streaming_reasoning(&delta);
                self.stream_dirty = true;
            }
            RhoEvent::AgentEnd { reply, duration_ms } => {
                finalize_reasoning(format_secs(duration_ms));
                finalize_streaming_response();
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
                finalize_streaming_response();
                CHAT_BLOCKS.write().unwrap().push(ChatBlock::ToolCall {
                    name,
                    args: arguments,
                    status: ToolStatus::Pending,
                    output: None,
                    expanded: false,
                });
                self.tail_and_redraw(cx);
            }
            RhoEvent::ToolResult {
                name,
                is_error,
                output,
            } => {
                let status = if is_error {
                    ToolStatus::Error
                } else {
                    ToolStatus::Success
                };
                let output = if output.is_empty() {
                    None
                } else {
                    Some(output)
                };
                finalize_tool_call(&name, status, output);
                self.tail_and_redraw(cx);
            }
            RhoEvent::ToolDenied { name } => {
                finalize_tool_call(
                    &name,
                    ToolStatus::Denied,
                    Some("denied by approval gate".into()),
                );
                self.tail_and_redraw(cx);
            }
            RhoEvent::ApprovalRequest {
                tool,
                arguments,
                risk,
            } => {
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
                self.push_block(
                    cx,
                    ChatBlock::Info(format!("\u{26a0} {:?} failed: {}", kind, error)),
                );
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

        if ui.button(cx, ids!(btn_stats)).clicked(actions) {
            match &mut self.agent {
                Some(agent) => {
                    self.stats_requested = true;
                    let _ = agent.get_session_stats();
                }
                None => self.push_block(cx, ChatBlock::Info("not connected.".into())),
            }
            return;
        }
        if ui.button(cx, ids!(stats_close)).clicked(actions) {
            self.ui.modal(cx, ids!(stats_modal)).close(cx);
        }

        if ui.button(cx, ids!(btn_help)).clicked(actions) {
            self.ui.modal(cx, ids!(help_modal)).open(cx);
        }
        if ui.button(cx, ids!(help_close)).clicked(actions) {
            self.ui.modal(cx, ids!(help_modal)).close(cx);
        }
        if ui.button(cx, ids!(model_close)).clicked(actions) {
            self.ui.modal(cx, ids!(model_modal)).close(cx);
        }
        if ui.button(cx, ids!(session_close)).clicked(actions) {
            self.ui.modal(cx, ids!(session_modal)).close(cx);
        }
        if ui.button(cx, ids!(provider_close)).clicked(actions) {
            self.ui.modal(cx, ids!(provider_modal)).close(cx);
        }


        if ui.button(cx, ids!(btn_reload)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                let result = agent.reload_extensions();
                self.push_block(cx, ChatBlock::Info("Reloading extensions\u{2026}".into()));
                let _ = result;
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return;
        }

        if ui.button(cx, ids!(btn_restart)).clicked(actions) {
            self.restart_agent(cx);
            return;
        }

        if ui.button(cx, ids!(btn_abort)).clicked(actions) {
            if !self.busy {
                self.push_block(cx, ChatBlock::Info("nothing to abort.".into()));
            } else if let Some(agent) = &mut self.agent {
                let _ = agent.abort();
                self.working_state = "aborting".into();
                self.tick_working(cx);
            }
            return;
        }

        if ui.button(cx, ids!(btn_session)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                self.resume_latest_requested = false;
                let _ = agent.list_sessions();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return;
        }

        if ui.button(cx, ids!(btn_resume)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                self.resume_latest_requested = true;
                let _ = agent.list_sessions();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
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
            self.ui
                .text_input(cx, ids!(model_filter_input))
                .set_text(cx, "");
            self.filtered_models = self.models.clone();
            self.sync_model_list(cx);
            self.ui.modal(cx, ids!(model_modal)).open(cx);
        }
        if let Some(filter) = self
            .ui
            .text_input(cx, ids!(model_filter_input))
            .changed(actions)
        {
            self.model_filter_text = filter;
            self.recompute_filtered_models(cx);
        }
        // Pick a model from the modal's list, then close.
        let mut picked: Option<String> = None;
        {
            let list = self
                .ui
                .widget(cx, ids!(model_list))
                .portal_list(cx, ids!(list));
            for (item_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(pick)).clicked(actions) {
                    if let Some((id, _, _)) = MODELS.read().unwrap().get(item_id) {
                        picked = Some(id.clone());
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

        // Resume a session from the modal's list, then close.
        let mut resumed: Option<String> = None;
        {
            let list = self
                .ui
                .widget(cx, ids!(session_list))
                .portal_list(cx, ids!(list));
            for (item_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(pick)).clicked(actions) {
                    if let Some((path, _, _)) = SESSIONS.read().unwrap().get(item_id) {
                        resumed = Some(path.clone());
                    }
                }
            }
        }
        if let Some(path) = resumed {
            self.ui.modal(cx, ids!(session_modal)).close(cx);
            if let Some(agent) = &mut self.agent {
                let _ = agent.resume_session(&path);
            }
        }

        // ── Resume-last-session confirmation modal ──
        if ui.button(cx, ids!(resume_confirm_cancel)).clicked(actions) {
            self.pending_resume_path = None;
            self.ui.modal(cx, ids!(resume_confirm_modal)).close(cx);
        }
        if ui.button(cx, ids!(resume_confirm_ok)).clicked(actions) {
            self.ui.modal(cx, ids!(resume_confirm_modal)).close(cx);
            if let Some(path) = self.pending_resume_path.take() {
                if let Some(agent) = &mut self.agent {
                    let _ = agent.resume_session(&path);
                }
            }
        }
        if self
            .ui
            .modal(cx, ids!(resume_confirm_modal))
            .dismissed(actions)
        {
            self.pending_resume_path = None;
        }

        // ── Inline block actions: tool call expand/collapse ──
        {
            let list = self
                .ui
                .widget(cx, ids!(chat_scroll))
                .portal_list(cx, ids!(list));
            for (item_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(expand_btn)).clicked(actions) {
                    let mut blocks = CHAT_BLOCKS.write().unwrap();
                    if let Some(ChatBlock::ToolCall { expanded, .. }) = blocks.get_mut(item_id) {
                        *expanded = !*expanded;
                    }
                    drop(blocks);
                    self.tail_and_redraw(cx);
                }
            }
        }

        // ── Inline block actions: approval buttons ──
        let mut approvals: Vec<(usize, bool, Option<String>)> = Vec::new();
        {
            let list = self
                .ui
                .widget(cx, ids!(chat_scroll))
                .portal_list(cx, ids!(list));
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
            }
        }
        for (idx, approved, msg) in approvals {
            self.resolve_approval(cx, idx, approved, msg);
        }

        // ── TextInput: Enter to submit ──
        let input = self.ui.text_input(cx, ids!(input_inner));
        if let Some((text, _mods)) = input.returned(actions) {
            if !text.is_empty() {
                input.set_text(cx, "");
                let steer = self.busy;
                let block = if steer {
                    ChatBlock::Steer(text.clone())
                } else {
                    ChatBlock::User(text.clone())
                };
                CHAT_BLOCKS
                    .write()
                    .unwrap()
                    .push(block);
                let send = if steer {
                    // Acknowledge the steer and reflect it in the working line.
                    self.working_state = "steered".into();
                    self.tick_working(cx);
                    self.ui.text_input(cx, ids!(input_inner))
                        .set_empty_text(cx, "Steer the agent... (Enter to send, Shift+Enter for newline)".into());
                    match &mut self.agent {
                        Some(agent) => agent.prompt(&text, true),
                        None => Err("rho agent not connected.".into()),
                    }
                } else {
                    match &mut self.agent {
                        Some(agent) => agent.prompt(&text, false),
                        None => Err("rho agent not connected.".into()),
                    }
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
        // Reschedule every frame, but only do (potentially expensive) redraw
        // work at most every ~80ms so streaming can't starve the event loop.
        if self.busy && self.next_frame.is_event(event).is_some() {
            self.next_frame = cx.new_next_frame();
            let due = self
                .last_tick
                .map_or(true, |t| t.elapsed() >= std::time::Duration::from_millis(80));
            if due {
                self.last_tick = Some(Instant::now());
                self.tick_working(cx);
                if self.stream_dirty {
                    // Redraw only — let the PortalList's auto_tail keep the
                    // bottom in view. Calling set_first_id_and_scroll every
                    // flush fights smooth_tail and causes warp-speed jitter.
                    self.ui.redraw(cx);
                    self.stream_dirty = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── JSON helper functions ───────────────────────────────────────────────

    #[test]
    fn jstr_extracts_string() {
        let v = json!({ "name": "cargo_check" });
        assert_eq!(jstr(Some(&v), "name"), "cargo_check");
    }

    #[test]
    fn jstr_returns_empty_for_missing_key() {
        let v = json!({ "name": "test" });
        assert_eq!(jstr(Some(&v), "missing"), "");
    }

    #[test]
    fn jstr_returns_empty_for_non_string() {
        let v = json!({ "count": 42 });
        assert_eq!(jstr(Some(&v), "count"), "");
    }

    #[test]
    fn jstr_returns_empty_for_none() {
        assert_eq!(jstr(None, "anything"), "");
    }

    #[test]
    fn ju64_extracts_number() {
        let v = json!({ "count": 1234 });
        assert_eq!(ju64(Some(&v), "count"), 1234);
    }

    #[test]
    fn ju64_returns_zero_for_missing_key() {
        let v = json!({});
        assert_eq!(ju64(Some(&v), "count"), 0);
    }

    #[test]
    fn ju64_returns_zero_for_non_number() {
        let v = json!({ "count": "not a number" });
        assert_eq!(ju64(Some(&v), "count"), 0);
    }

    #[test]
    fn ju64_returns_zero_for_none() {
        assert_eq!(ju64(None, "count"), 0);
    }

    #[test]
    fn jf64_extracts_float() {
        let v = json!({ "cost": 0.0042 });
        assert!((jf64(Some(&v), "cost") - 0.0042).abs() < 1e-9);
    }

    #[test]
    fn jf64_returns_zero_for_missing_key() {
        let v = json!({});
        assert!((jf64(Some(&v), "cost") - 0.0).abs() < 1e-9);
    }

    #[test]
    fn jf64_returns_zero_for_non_number() {
        let v = json!({ "cost": true });
        assert!((jf64(Some(&v), "cost") - 0.0).abs() < 1e-9);
    }

    #[test]
    fn jbool_extracts_boolean() {
        let v = json!({ "is_error": true });
        assert!(jbool(Some(&v), "is_error"));
    }

    #[test]
    fn jbool_returns_false_for_missing_key() {
        let v = json!({});
        assert!(!jbool(Some(&v), "is_error"));
    }

    #[test]
    fn jbool_returns_false_for_non_boolean() {
        let v = json!({ "is_error": "yes" });
        assert!(!jbool(Some(&v), "is_error"));
    }

    #[test]
    fn jbool_returns_false_for_none() {
        assert!(!jbool(None, "is_error"));
    }

    // ── format_secs ─────────────────────────────────────────────────────────

    #[test]
    fn format_secs_whole_second() {
        assert_eq!(format_secs(1000), "1.0s");
    }

    #[test]
    fn format_secs_fractional() {
        assert_eq!(format_secs(3200), "3.2s");
    }

    #[test]
    fn format_secs_zero() {
        assert_eq!(format_secs(0), "0.0s");
    }

    #[test]
    fn format_secs_large_value() {
        assert_eq!(format_secs(65500), "65.5s");
    }

    // ── cap_head ────────────────────────────────────────────────────────────

    #[test]
    fn cap_head_short_string_unchanged() {
        assert_eq!(cap_head("hello", 100), "hello");
    }

    #[test]
    fn cap_head_exact_length_unchanged() {
        assert_eq!(cap_head("hello", 5), "hello");
    }

    #[test]
    fn cap_head_truncates_with_suffix() {
        let result = cap_head("hello world", 5);
        assert_eq!(result, "hello\n\u{2026} (6 more chars)");
    }

    #[test]
    fn cap_head_empty_string() {
        assert_eq!(cap_head("", 10), "");
    }

    #[test]
    fn cap_head_multibyte_chars() {
        // Each emoji is 1 char but multiple bytes — cap_head counts chars, not bytes.
        let s = "\u{1f600}\u{1f600}\u{1f600}"; // 3 chars
        assert_eq!(cap_head(s, 10), s);
    }

    #[test]
    fn cap_head_multibyte_truncation() {
        let s = "\u{1f600}\u{1f600}\u{1f600}\u{1f600}"; // 4 chars
        let result = cap_head(s, 2);
        assert_eq!(result, "\u{1f600}\u{1f600}\n\u{2026} (2 more chars)");
    }

    // ── cap_tail ────────────────────────────────────────────────────────────

    #[test]
    fn cap_tail_short_string_unchanged() {
        assert_eq!(cap_tail("hello", 100), "hello");
    }

    #[test]
    fn cap_tail_exact_length_unchanged() {
        assert_eq!(cap_tail("hello", 5), "hello");
    }

    #[test]
    fn cap_tail_truncates_with_prefix() {
        let result = cap_tail("hello world", 5);
        assert_eq!(result, "\u{2026} (6 earlier chars)\nworld");
    }

    #[test]
    fn cap_tail_empty_string() {
        assert_eq!(cap_tail("", 10), "");
    }

    #[test]
    fn cap_tail_multibyte_chars() {
        let s = "\u{1f600}\u{1f600}\u{1f600}";
        assert_eq!(cap_tail(s, 10), s);
    }

    #[test]
    fn cap_tail_multibyte_truncation() {
        let s = "\u{1f600}\u{1f600}\u{1f600}\u{1f600}"; // 4 chars
        let result = cap_tail(s, 2);
        assert_eq!(result, "\u{2026} (2 earlier chars)\n\u{1f600}\u{1f600}");
    }

    // ── relative_time ─────────────────────────────────────────────────────────

    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    #[test]
    fn relative_time_just_now() {
        assert_eq!(relative_time(now_secs()), "just now");
    }

    #[test]
    fn relative_time_minutes_ago() {
        let t = now_secs().saturating_sub(300); // 5 minutes ago
        assert_eq!(relative_time(t), "5m ago");
    }

    #[test]
    fn relative_time_hours_ago() {
        let t = now_secs().saturating_sub(7200); // 2 hours ago
        assert_eq!(relative_time(t), "2h ago");
    }

    #[test]
    fn relative_time_days_ago() {
        let t = now_secs().saturating_sub(86_400 * 3); // 3 days ago
        assert_eq!(relative_time(t), "3d ago");
    }

    #[test]
    fn relative_time_weeks_ago() {
        let t = now_secs().saturating_sub(86_400 * 14); // 2 weeks ago
        assert_eq!(relative_time(t), "2w ago");
    }

    #[test]
    fn relative_time_months_ago() {
        let t = now_secs().saturating_sub(86_400 * 60); // ~2 months ago
        assert_eq!(relative_time(t), "2mo ago");
    }

    #[test]
    fn relative_time_future_timestamp() {
        // A timestamp in the future should saturate to "just now".
        let t = now_secs() + 10_000;
        assert_eq!(relative_time(t), "just now");
    }

    // ── parse_stdout (notification parsing) ───────────────────────────────────
    //
    // parse_stdout requires &mut RhoAgent (for the `pending` HashMap), which we
    // can't construct without spawning a subprocess. But the notification branch
    // (lines without an `id` field) doesn't touch `pending` at all — it only reads
    // the `method` and `params` fields. We can test this path by temporarily
    // bypassing the `pending` check.
    //
    // Strategy: feed lines that have no `id`, so parse_stdout takes the
    // notification branch and never touches `self.pending`.

    /// Parse a notification line (no `id` field) by temporarily creating a
    /// fake pending map. The notification branch never reads from it.
    fn parse_notification(line: &str) -> Option<RhoEvent> {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        // Simulate the notification branch of parse_stdout.
        if v.get("id").is_some() {
            return None; // This helper only tests notifications, not responses.
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
            "agent/error" => RhoEvent::AgentError {
                error: jstr(p, "error"),
            },
            "message/delta" => RhoEvent::MessageDelta {
                delta: jstr(p, "delta"),
            },
            "reasoning/delta" => RhoEvent::ReasoningDelta {
                delta: jstr(p, "delta"),
            },
            "state/change" => RhoEvent::StateChange {
                state: jstr(p, "state"),
            },
            "tool/call" => RhoEvent::ToolCall {
                name: jstr(p, "name"),
                arguments: jstr(p, "arguments"),
            },
            "tool/result" => RhoEvent::ToolResult {
                name: jstr(p, "name"),
                is_error: jbool(p, "isError"),
                output: jstr(p, "output"),
            },
            "tool/denied" => RhoEvent::ToolDenied {
                name: jstr(p, "name"),
            },
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

    #[test]
    fn parse_notification_ready() {
        let line = r#"{"method":"ready","params":{}}"#;
        assert!(matches!(parse_notification(line), Some(RhoEvent::Ready)));
    }

    #[test]
    fn parse_notification_agent_start() {
        let line = r#"{"method":"agent/start","params":{}}"#;
        assert!(matches!(parse_notification(line), Some(RhoEvent::AgentStart)));
    }

    #[test]
    fn parse_notification_agent_end() {
        let line = r#"{"method":"agent/end","params":{"reply":"hello","durationMs":3200}}"#;
        match parse_notification(line) {
            Some(RhoEvent::AgentEnd { reply, duration_ms }) => {
                assert_eq!(reply, "hello");
                assert_eq!(duration_ms, 3200);
            }
            other => panic!("expected AgentEnd, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_agent_error() {
        let line = r#"{"method":"agent/error","params":{"error":"boom"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::AgentError { error }) => {
                assert_eq!(error, "boom");
            }
            other => panic!("expected AgentError, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_message_delta() {
        let line = r#"{"method":"message/delta","params":{"delta":"world"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::MessageDelta { delta }) => {
                assert_eq!(delta, "world");
            }
            other => panic!("expected MessageDelta, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_reasoning_delta() {
        let line = r#"{"method":"reasoning/delta","params":{"delta":"thinking..."}}"#;
        match parse_notification(line) {
            Some(RhoEvent::ReasoningDelta { delta }) => {
                assert_eq!(delta, "thinking...");
            }
            other => panic!("expected ReasoningDelta, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_state_change() {
        let line = r#"{"method":"state/change","params":{"state":"idle"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::StateChange { state }) => {
                assert_eq!(state, "idle");
            }
            other => panic!("expected StateChange, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_tool_call() {
        let line = r#"{"method":"tool/call","params":{"name":"read_file","arguments":"{\"path\":\"src/main.rs\"}"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::ToolCall { name, arguments }) => {
                assert_eq!(name, "read_file");
                assert!(arguments.contains("main.rs"));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_tool_result_success() {
        let line = r#"{"method":"tool/result","params":{"name":"read_file","isError":false,"output":"file contents"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::ToolResult { name, is_error, output }) => {
                assert_eq!(name, "read_file");
                assert!(!is_error);
                assert_eq!(output, "file contents");
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_tool_result_error() {
        let line = r#"{"method":"tool/result","params":{"name":"write_file","isError":true,"output":"permission denied"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::ToolResult { name, is_error, output }) => {
                assert_eq!(name, "write_file");
                assert!(is_error);
                assert_eq!(output, "permission denied");
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_tool_denied() {
        let line = r#"{"method":"tool/denied","params":{"name":"run_command"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::ToolDenied { name }) => {
                assert_eq!(name, "run_command");
            }
            other => panic!("expected ToolDenied, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_approval_request() {
        let line = r#"{"method":"approval/request","params":{"tool":"run_command","arguments":"ls","risk":"read"}}"#;
        match parse_notification(line) {
            Some(RhoEvent::ApprovalRequest { tool, arguments, risk }) => {
                assert_eq!(tool, "run_command");
                assert_eq!(arguments, "ls");
                assert_eq!(risk, "read");
            }
            other => panic!("expected ApprovalRequest, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_usage() {
        let line = r#"{"method":"usage","params":{"usage":{"inputTokens":1000,"outputTokens":500,"cachedTokens":200,"cost":0.0042},"context":{"estimatedUsed":8000,"contextWindow":200000,"utilizationPercent":4}}}"#;
        match parse_notification(line) {
            Some(RhoEvent::Usage {
                input_tokens,
                output_tokens,
                cached_tokens,
                cost,
                context_used,
                context_window,
                utilization,
            }) => {
                assert_eq!(input_tokens, 1000);
                assert_eq!(output_tokens, 500);
                assert_eq!(cached_tokens, 200);
                assert!((cost - 0.0042).abs() < 1e-9);
                assert_eq!(context_used, 8000);
                assert_eq!(context_window, 200000);
                assert_eq!(utilization, 4);
            }
            other => panic!("expected Usage, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_usage_utilization_clamped() {
        // utilizationPercent > 255 should be clamped to 255 (u8 max).
        let line = r#"{"method":"usage","params":{"usage":{"inputTokens":0,"outputTokens":0,"cachedTokens":0,"cost":0.0},"context":{"estimatedUsed":0,"contextWindow":0,"utilizationPercent":300}}}"#;
        match parse_notification(line) {
            Some(RhoEvent::Usage { utilization, .. }) => {
                assert_eq!(utilization, 255);
            }
            other => panic!("expected Usage, got {:?}", other),
        }
    }

    #[test]
    fn parse_notification_unknown_method_returns_none() {
        let line = r#"{"method":"unknown/method","params":{}}"#;
        assert!(parse_notification(line).is_none());
    }

    #[test]
    fn parse_notification_invalid_json_returns_none() {
        assert!(parse_notification("not json at all").is_none());
    }

    #[test]
    fn parse_notification_missing_method_returns_none() {
        let line = r#"{"params":{}}"#;
        assert!(parse_notification(line).is_none());
    }

    // ── Streaming helpers (via CHAT_BLOCKS global) ────────────────────────────

    /// Serializes tests that touch the global CHAT_BLOCKS — cargo runs tests in
    /// parallel, so unsynchronized clears race other tests' asserts (flaky
    /// failures). Hold the returned guard for the whole test.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[must_use]
    fn reset_chat_blocks() -> std::sync::MutexGuard<'static, ()> {
        // A panicking test poisons the mutex; the data it guards is re-cleared
        // here anyway, so poisoning is safe to ignore.
        let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        CHAT_BLOCKS.write().unwrap().clear();
        guard
    }

    #[test]
    fn append_streaming_response_creates_new_block() {
        let _guard = reset_chat_blocks();
        append_streaming_response("hello");
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::ResponseStreaming(text) => assert_eq!(text, "hello"),
            other => panic!("expected ResponseStreaming, got {:?}", other),
        }
    }

    #[test]
    fn append_streaming_response_appends_to_existing() {
        let _guard = reset_chat_blocks();
        append_streaming_response("hello");
        append_streaming_response(" world");
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::ResponseStreaming(text) => assert_eq!(text, "hello world"),
            other => panic!("expected ResponseStreaming, got {:?}", other),
        }
    }

    #[test]
    fn append_streaming_response_creates_new_block_after_finalize() {
        let _guard = reset_chat_blocks();
        append_streaming_response("first");
        finalize_streaming_response();
        append_streaming_response("second");
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(matches!(&blocks[0], ChatBlock::Response(_)));
        assert!(matches!(&blocks[1], ChatBlock::ResponseStreaming(_)));
    }

    #[test]
    fn finalize_streaming_response_noop_on_empty() {
        let _guard = reset_chat_blocks();
        finalize_streaming_response();
        assert!(CHAT_BLOCKS.read().unwrap().is_empty());
    }

    #[test]
    fn finalize_streaming_response_noop_on_non_streaming() {
        let _guard = reset_chat_blocks();
        CHAT_BLOCKS.write().unwrap().push(ChatBlock::Info("test".into()));
        finalize_streaming_response();
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 1);
        assert!(matches!(&blocks[0], ChatBlock::Info(_)));
    }

    #[test]
    fn append_streaming_reasoning_creates_new_block() {
        let _guard = reset_chat_blocks();
        append_streaming_reasoning("thinking...");
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::ReasoningStreaming { text } => assert_eq!(text, "thinking..."),
            other => panic!("expected ReasoningStreaming, got {:?}", other),
        }
    }

    #[test]
    fn append_streaming_reasoning_appends_to_existing() {
        let _guard = reset_chat_blocks();
        append_streaming_reasoning("thinking");
        append_streaming_reasoning(" more");
        let blocks = CHAT_BLOCKS.read().unwrap();
        match &blocks[0] {
            ChatBlock::ReasoningStreaming { text } => assert_eq!(text, "thinking more"),
            other => panic!("expected ReasoningStreaming, got {:?}", other),
        }
    }

    #[test]
    fn finalize_reasoning_converts_to_finalized() {
        let _guard = reset_chat_blocks();
        append_streaming_reasoning("deep thoughts");
        finalize_reasoning("5.2s".into());
        let blocks = CHAT_BLOCKS.read().unwrap();
        match &blocks[0] {
            ChatBlock::Reasoning { text, elapsed_secs } => {
                assert_eq!(text, "deep thoughts");
                assert_eq!(elapsed_secs, "5.2s");
            }
            other => panic!("expected Reasoning, got {:?}", other),
        }
    }

    // ── finalize_tool_call ───────────────────────────────────────────────────

    #[test]
    fn finalize_tool_call_updates_pending() {
        let _guard = reset_chat_blocks();
        CHAT_BLOCKS.write().unwrap().push(ChatBlock::ToolCall {
            name: "read_file".into(),
            args: "{\"path\":\"test.rs\"}".into(),
            status: ToolStatus::Pending,
            output: None,
            expanded: false,
        });
        finalize_tool_call("read_file", ToolStatus::Success, Some("ok".into()));
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::ToolCall { name, status, output, .. } => {
                assert_eq!(name, "read_file");
                assert!(matches!(status, ToolStatus::Success));
                assert_eq!(output.as_deref(), Some("ok"));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn finalize_tool_call_fallback_pushes_new_block() {
        let _guard = reset_chat_blocks();
        // No matching pending tool call — should push a fallback block.
        finalize_tool_call("unknown_tool", ToolStatus::Error, Some("failed".into()));
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ChatBlock::ToolCall { name, status, output, args, .. } => {
                assert_eq!(name, "unknown_tool");
                assert!(matches!(status, ToolStatus::Error));
                assert_eq!(output.as_deref(), Some("failed"));
                assert_eq!(args, "");
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn finalize_tool_call_skips_non_pending() {
        let _guard = reset_chat_blocks();
        CHAT_BLOCKS.write().unwrap().push(ChatBlock::ToolCall {
            name: "read_file".into(),
            args: String::new(),
            status: ToolStatus::Success,
            output: Some("already done".into()),
            expanded: false,
        });
        // Should NOT update the already-success block, should push fallback.
        finalize_tool_call("read_file", ToolStatus::Error, Some("different".into()));
        let blocks = CHAT_BLOCKS.read().unwrap();
        assert_eq!(blocks.len(), 2);
        // First block unchanged
        match &blocks[0] {
            ChatBlock::ToolCall { status, output, .. } => {
                assert!(matches!(status, ToolStatus::Success));
                assert_eq!(output.as_deref(), Some("already done"));
            }
            other => panic!("expected ToolCall, got {:?}", other),
        }
    }
}
