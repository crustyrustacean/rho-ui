pub use makepad_widgets;

use makepad_widgets::*;
use std::sync::RwLock;

app_main!(App);

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
    /// A system/info message (e.g. "switched model", "resumed session").
    Info(String),
}

#[derive(Clone, Debug)]
enum ToolStatus {
    Pending,
    Success,
    Error,
}

static CHAT_BLOCKS: RwLock<Vec<ChatBlock>> = RwLock::new(Vec::new());

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
                                w.label(cx, ids!(msg)).set_text(cx, text);
                                w.draw_all_unscoped(cx);
                            }
                            ChatBlock::ToolCall { name, args, status, output } => {
                                let (template, status_text) = match status {
                                    ToolStatus::Success => (id!(ToolDone), "✓ done"),
                                    ToolStatus::Pending => (id!(ToolRun), "⟳ running"),
                                    ToolStatus::Error => (id!(ToolFail), "✗ failed"),
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
                                text: "12.3k  8.7k R4.2k $0.042 23%/200k(auto)"
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
            self.push_block(cx, ChatBlock::Info("abort requested".into()));
            return;
        }

        if ui.button(cx, ids!(btn_session)).clicked(actions) {
            self.push_block(cx, ChatBlock::Info("Session picker not yet connected to backend.".into()));
            return;
        }

        if ui.button(cx, ids!(btn_model)).clicked(actions) {
            self.push_block(cx, ChatBlock::Info("Model picker not yet connected to backend.".into()));
            return;
        }

        if ui.button(cx, ids!(btn_resume)).clicked(actions) {
            self.push_block(cx, ChatBlock::Info("Resume not yet connected to backend.".into()));
            return;
        }

        if ui.button(cx, ids!(btn_providers)).clicked(actions) {
            self.push_block(cx, ChatBlock::Info("Providers picker not yet connected to backend.".into()));
            return;
        }

        // ── TextInput: Enter to submit ──
        let input = self.ui.text_input(cx, ids!(input_inner));
        if let Some((text, _mods)) = input.returned(actions) {
            if !text.is_empty() {
                input.set_text(cx, "");
                self.push_block(cx, ChatBlock::User(text.clone()));
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

        // On startup, seed the scrollback with sample content and tail to it.
        if let Event::Startup = event {
            *CHAT_BLOCKS.write().unwrap() = vec![
                ChatBlock::User("Review this project and gain context on it.".into()),
                ChatBlock::Reasoning {
                    text: "Let me explore the project structure and read the key files to understand what's going on.".into(),
                    elapsed_secs: "2.3s".into(),
                },
                ChatBlock::Response("I've reviewed the project. It's a GUI frontend built with makepad, using the Script DSL. The layout is a three-region chat-style window with a header, middle content area, and footer.".into()),
                ChatBlock::ToolCall {
                    name: "read_file".into(),
                    args: "src/main.rs".into(),
                    status: ToolStatus::Success,
                    output: Some("pub use makepad_widgets;\nuse makepad_widgets::*;\n\napp_main!(App);".into()),
                },
                ChatBlock::ToolCall {
                    name: "cargo_check".into(),
                    args: "".into(),
                    status: ToolStatus::Pending,
                    output: None,
                },
                ChatBlock::ToolCall {
                    name: "edit_file".into(),
                    args: "src/main.rs".into(),
                    status: ToolStatus::Error,
                    output: Some("error: hash mismatch at line 42".into()),
                },
                ChatBlock::User("Can you fix the compilation error?".into()),
                ChatBlock::ReasoningStreaming {
                    text: "The hash mismatch is because the file changed since it was last read. I need to re-read the file to get fresh hashes, then retry the edit.".into(),
                },
            ];
            self.tail_and_redraw(cx);
        }
    }
}
