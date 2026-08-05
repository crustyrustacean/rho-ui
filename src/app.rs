// ── The app, in Rust ───────────────────────────────────────────────────────
// App holds no conversation state itself — it mutates the global CHAT_BLOCKS
// and tells the PortalList to tail + redraw.

use crate::makepad_widgets::*;
use crate::trace;
use std::time::Instant;

use crate::util::{formatting::*, json::*};
use crate::agent::{RhoAgent, RhoEvent, RequestKind};
use crate::ui::widgets;
use crate::ui::widgets::model_list::ModelEntry;
use crate::ui::widgets::session_list::SessionEntry;
use crate::ui::widgets::provider_list::ProviderEntry;
use crate::chat::{store as chat_store, ChatBlock, ToolStatus, ApprovalResolution};

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
    context_modal_requested: bool,
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
    steer_count: u32,
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
        let len = chat_store::len();
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
        chat_store::push_after_finalizing_response(block);
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
        self.steer_count = 0;
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
            // Steering badge: show a count next to the spinner when
            // mid-turn steers have been queued.
            let badge = if self.steer_count > 0 {
                format!("  \u{2197} {} steered", self.steer_count)
            } else {
                String::new()
            };
            self.ui.label(cx, ids!(working_text)).set_text(
                cx,
                &format!("{} Working  {:.1}s  {}{}", ch, elapsed.as_secs_f64(), state, badge),
            );
        }
    }

    /// Write `filtered_models` into the picker modal's global and redraw it.
    fn sync_model_list(&self, cx: &mut Cx) {
        let current = self.current_model.as_deref();
        {
            let mapped: Vec<ModelEntry> = self.filtered_models.iter().map(|(id, provider)| {
                ModelEntry { id: id.clone(), provider: provider.clone(), is_current: Some(id.as_str()) == current }
            }).collect();
            widgets::model_list::set_models(mapped);
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
                let mut providers: Vec<ProviderEntry> = Vec::new();
                if let Some(arr) = result.get("providers").and_then(|x| x.as_array()) {
                    for p in arr {
                        providers.push(ProviderEntry {
                            name: jstr(Some(p), "name"),
                            reachable: jbool(Some(p), "reachable"),
                            active: jbool(Some(p), "active"),
                            is_external: jbool(Some(p), "isExternal"),
                        });
                    }
                }
                if providers.is_empty() {
                    self.push_block(cx, ChatBlock::Info("No providers configured.".into()));
                } else {
                    {
                        widgets::provider_list::set_providers(providers);
                    }
                    self.ui.redraw(cx);
                    self.ui.modal(cx, ids!(provider_modal)).open(cx);
                }
            }
            RequestKind::ListSessions => {
                let mut sessions: Vec<SessionEntry> = result
                    .get("sessions")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|s| {
                                SessionEntry {
                                    path: jstr(Some(s), "path"),
                                    mtime_secs: ju64(Some(s), "mtimeSecs"),
                                    entry_count: ju64(Some(s), "entryCount"),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Newest first (by modification time).
                sessions.sort_by_key(|b| std::cmp::Reverse(b.mtime_secs));
                if sessions.is_empty() {
                    self.push_block(cx, ChatBlock::Info("No previous sessions.".into()));
                } else if self.resume_latest_requested {
                    // "Resume Last": confirm the most recent session before resuming.
                    self.resume_latest_requested = false;
                    let entry = sessions[0].clone();
                    self.pending_resume_path = Some(entry.path.clone());
                    self.ui.label(cx, ids!(resume_confirm_meta)).set_text(
                        cx,
                        &format!("{} \u{00b7} {} entries", relative_time(entry.mtime_secs), entry.entry_count),
                    );
                    self.ui
                        .label(cx, ids!(resume_confirm_path))
                        .set_text(cx, &entry.path);
                    self.ui.modal(cx, ids!(resume_confirm_modal)).open(cx);
                } else {
                    {
                        widgets::session_list::set_sessions(sessions);
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
                // Context modal: same getSessionStats response feeds this modal
                // when the user opened it via the Context button.
                if self.context_modal_requested {
                    self.context_modal_requested = false;
                    self.ui
                        .label(cx, ids!(context_body))
                        .set_text(cx, &format_session_stats(&result));
                    self.ui.modal(cx, ids!(context_modal)).open(cx);
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
            RequestKind::ListExtensions => {
                // Render the extensions list as a formatted string in a
                // scrollable modal. rho returns an `extensions` array with
                // at least a `name` field per entry; we show name + status.
                let body = if let Some(arr) = result.get("extensions").and_then(|x| x.as_array()) {
                    if arr.is_empty() {
                        "No extensions installed.".to_string()
                    } else {
                        arr.iter()
                            .map(|e| {
                                let name = jstr(Some(e), "name");
                                let status = jstr(Some(e), "status");
                                let tools = ju64(Some(e), "toolCount");
                                if status.is_empty() {
                                    if tools > 0 {
                                        format!("  {} ({} tools)", name, tools)
                                    } else {
                                        format!("  {}", name)
                                    }
                                } else {
                                    if tools > 0 {
                                        format!("  {} \u{2014} {} ({} tools)", name, status, tools)
                                    } else {
                                        format!("  {} \u{2014} {}", name, status)
                                    }
                                }
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    }
                } else {
                    // Unknown shape — dump the raw JSON so nothing is hidden.
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                };
                self.ui
                    .label(cx, ids!(extensions_body))
                    .set_text(cx, &body);
                self.ui.modal(cx, ids!(extensions_modal)).open(cx);
            }
            RequestKind::Compact => {
                self.push_block(cx, ChatBlock::Info("\u{2713} compacted".into()));
                if let Some(agent) = &mut self.agent {
                    let _ = agent.get_session_stats();
                }
            }
            RequestKind::Clear => {
                chat_store::clear();
                self.push_block(cx, ChatBlock::Info("\u{2713} conversation cleared".into()));
                if let Some(agent) = &mut self.agent {
                    let _ = agent.get_session_stats();
                }
            }
            RequestKind::NewSession => {
                chat_store::clear();
                self.push_block(cx, ChatBlock::Info("\u{2713} new session".into()));
                if let Some(agent) = &mut self.agent {
                    let _ = agent.get_session_stats();
                }
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
        chat_store::resolve_approval(index, resolution);
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
                chat_store::append_streaming_response(&delta);
                self.stream_dirty = true;
            }
            RhoEvent::ReasoningDelta { delta } => {
                chat_store::finalize_streaming_response();
                chat_store::append_streaming_reasoning(&delta);
                self.stream_dirty = true;
            }
            RhoEvent::AgentEnd { reply, duration_ms } => {
                chat_store::finalize_reasoning(format_secs(duration_ms));
                chat_store::finalize_streaming_response();
                // If nothing streamed, the final reply is our only text.
                chat_store::push_final_response_if_missing(reply);
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
                chat_store::finalize_streaming_response();
                chat_store::push_pending_tool_call(name, arguments);
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
                chat_store::finalize_tool_call(&name, status, output);
                self.tail_and_redraw(cx);
            }
            RhoEvent::ToolDenied { name } => {
                chat_store::finalize_tool_call(
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
                chat_store::push_approval(tool, arguments, risk);
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

impl App {
    /// Handle the menu-bar buttons and modal-close buttons. Returns `true` if a
    /// button consumed the event and processing should stop (early-return).
    fn handle_menu_buttons(&mut self, cx: &mut Cx, actions: &Actions) -> bool {
        let ui = self.ui.clone();

        if ui.button(cx, ids!(btn_quit)).clicked(actions) {
            cx.quit();
            return true;
        }
        if ui.button(cx, ids!(btn_stats)).clicked(actions) {
            match &mut self.agent {
                Some(agent) => {
                    self.stats_requested = true;
                    let _ = agent.get_session_stats();
                }
                None => self.push_block(cx, ChatBlock::Info("not connected.".into())),
            }
            return true;
        }
        if ui.button(cx, ids!(btn_context)).clicked(actions) {
            match &mut self.agent {
                Some(agent) => {
                    self.context_modal_requested = true;
                    let _ = agent.get_session_stats();
                }
                None => self.push_block(cx, ChatBlock::Info("not connected.".into())),
            }
            return true;
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
        if ui.button(cx, ids!(extensions_close)).clicked(actions) {
            self.ui.modal(cx, ids!(extensions_modal)).close(cx);
        }
        if ui.button(cx, ids!(context_close)).clicked(actions) {
            self.ui.modal(cx, ids!(context_modal)).close(cx);
        }
        if ui.button(cx, ids!(context_compact)).clicked(actions) {
            self.ui.modal(cx, ids!(context_modal)).close(cx);
            if let Some(agent) = &mut self.agent {
                let _ = agent.compact();
            }
        }
        if ui.button(cx, ids!(context_clear)).clicked(actions) {
            self.ui.modal(cx, ids!(context_modal)).close(cx);
            if let Some(agent) = &mut self.agent {
                let _ = agent.clear();
            }
        }
        if ui.button(cx, ids!(context_new_session)).clicked(actions) {
            self.ui.modal(cx, ids!(context_modal)).close(cx);
            if let Some(agent) = &mut self.agent {
                let _ = agent.new_session();
            }
        }
        if ui.button(cx, ids!(btn_providers)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                let _ = agent.list_providers();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return true;
        }
        if ui.button(cx, ids!(btn_reload)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                let result = agent.reload_extensions();
                self.push_block(cx, ChatBlock::Info("Reloading extensions\u{2026}".into()));
                let _ = result;
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return true;
        }
        if ui.button(cx, ids!(btn_restart)).clicked(actions) {
            self.restart_agent(cx);
            return true;
        }
        if ui.button(cx, ids!(btn_abort)).clicked(actions) {
            if !self.busy {
                self.push_block(cx, ChatBlock::Info("nothing to abort.".into()));
            } else if let Some(agent) = &mut self.agent {
                let _ = agent.abort();
                self.working_state = "aborting".into();
                self.tick_working(cx);
            }
            return true;
        }
        if ui.button(cx, ids!(btn_session)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                self.resume_latest_requested = false;
                let _ = agent.list_sessions();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return true;
        }
        if ui.button(cx, ids!(btn_resume)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                self.resume_latest_requested = true;
                let _ = agent.list_sessions();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return true;
        }
        if ui.button(cx, ids!(btn_extensions)).clicked(actions) {
            if let Some(agent) = &mut self.agent {
                let _ = agent.list_extensions();
            } else {
                self.push_block(cx, ChatBlock::Info("not connected.".into()));
            }
            return true;
        }
        false
    }

    /// Model picker modal: open, filter, and pick a model from the list.
    fn handle_model_picker(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(btn_model)).clicked(actions) {
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
        let picked = widgets::picked_items(&self.ui, cx, actions, ids!(model_list));
        if let Some(&item_id) = picked.first() {
            if let Some(entry) = widgets::model_list::get_model(item_id) {
                let model = entry.id.clone();
                self.ui.modal(cx, ids!(model_modal)).close(cx);
                if self.current_model.as_deref() != Some(model.as_str()) {
                    if let Some(agent) = &mut self.agent {
                        let _ = agent.set_model(&model);
                    }
                }
            }
        }
    }

    /// Session picker + resume-last-session confirmation modal.
    fn handle_session_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // Pick a session from the modal's list, then close.
        let resumed = widgets::picked_items(&self.ui, cx, actions, ids!(session_list));
        if let Some(&item_id) = resumed.first() {
            if let Some(entry) = widgets::session_list::get_session(item_id) {
                let path = entry.path.clone();
                self.ui.modal(cx, ids!(session_modal)).close(cx);
                if let Some(agent) = &mut self.agent {
                    let _ = agent.resume_session(&path);
                }
            }
        }
        // ── Resume-last-session confirmation modal ──
        if self.ui.button(cx, ids!(resume_confirm_cancel)).clicked(actions) {
            self.pending_resume_path = None;
            self.ui.modal(cx, ids!(resume_confirm_modal)).close(cx);
        }
        if self.ui.button(cx, ids!(resume_confirm_ok)).clicked(actions) {
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
    }

    /// Inline chat-block actions: expand/collapse + approval buttons.
    fn handle_chat_block_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        // ── Expand/collapse (tool calls + responses) ──
        {
            let list = self
                .ui
                .widget(cx, ids!(chat_scroll))
                .portal_list(cx, ids!(list));
            for (item_id, item) in list.items_with_actions(actions) {
                if item.button(cx, ids!(expand_btn)).clicked(actions) {
                    chat_store::toggle_expand(item_id);
                    self.tail_and_redraw(cx);
                }
            }
        }
        // ── Approval buttons ──
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
    }

    /// TextInput: Enter to submit (or steer while busy).
    fn handle_text_input(&mut self, cx: &mut Cx, actions: &Actions) {
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
                chat_store::push(block);
                let send = if steer {
                    self.steer_count += 1;
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
                    chat_store::push(ChatBlock::Info(format!("\u{26a0} {}", e)));
                }
                self.tail_and_redraw(cx);
            }
            // Keep typing: submitting (or pressing Enter on an empty box) can drop key
            // focus, so re-assert it on the input every time.
            input.set_key_focus(cx);
        }
    }
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.handle_menu_buttons(cx, actions) {
            return;
        }
        self.handle_model_picker(cx, actions);
        self.handle_session_actions(cx, actions);
        self.handle_chat_block_actions(cx, actions);
        self.handle_text_input(cx, actions);
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::ui::script::script_mod(vm)
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
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_millis(80));
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