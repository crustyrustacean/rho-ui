use crate::App;
use crate::ui::{ChatScroll, ModelList, ProviderList, SessionList};
use crate::makepad_widgets::*;

// ── The UI, in Script DSL ───────────────────────────────────────────────────
// Layout (top → bottom):
//   1. Title bar (Fit) — "rho" branding + subtitle.
//   2. Menu bar (Fit) — row of buttons: Session, Resume Last, Context, Model,
//      Providers, Extensions, Reload, Restart, Abort, Stats, Help, Quit.
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
                flow: Down spacing: 4
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
                    // Groups: Session | Config | Process | Info
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
                        btn_context := Button{
                            text: "Context"
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
                        btn_extensions := Button{
                            text: "Extensions"
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
                            width: 500
                            height: 520
                            flow: Down

                            SolidView{
                                width: Fill height: Fill
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Session stats"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                stats_scroll := ScrollYView{
                                    width: Fill height: Fill
                                    padding: Inset{right: 10}
                                    stats_body := Label{
                                        width: Fill
                                        text: ""
                                        draw_text.color: #xcacaca
                                        draw_text.text_style: theme.font_code{font_size: 12}
                                    }
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
                            width: 680
                            height: 520
                            flow: Down

                            SolidView{
                                width: Fill height: Fill
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "rho — quick reference"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                help_scroll := ScrollYView{
                                    width: Fill height: Fill
                                    padding: Inset{right: 10}
                                    Label{
                                        width: Fill
                                        height: Fit
                                        text: "Type a message and press Enter to chat with the agent.\n\nMenu buttons:\n  Session — list and resume previous sessions\n  Resume Last — quickly resume the most recent session\n  Model — pick a model from the scrollable list\n  Providers — view configured providers and their status\n  Reload — reload extensions from disk (picks up new .rho/extensions/*.ts)\n  Context — compact, clear, or start a new session, with live stats
  Restart — kill and re-spawn the rho subprocess (use if it's stuck or unresponsive)\n  Abort — cancel the current agent turn\n  Help — this dialog\n  Quit — exit rho\n\nInput: Enter sends, Shift+Enter inserts a newline.\n\nWhile the agent is working, the input placeholder changes to 'Steer the agent...' and your message is sent as a mid-turn steering prompt instead of starting a new turn. Steering messages appear with a distinct background and are reflected in the working line.\n\nTool calls that need approval show Approve / Deny / Redirect buttons inline.\n\nTool output longer than 10000 characters is truncated — click Expand to see the full output, Collapse to hide it again."
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    btn_docs := Button{
                                        text: "Documentation"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
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
                    // ── Extensions modal ──
                    extensions_modal := Modal{
                        content +: {
                            width: 500
                            height: 520
                            flow: Down

                            SolidView{
                                width: Fill height: Fill
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Extensions"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                ScrollYView{
                                    width: Fill height: Fill
                                    padding: Inset{right: 10}
                                    extensions_body := Label{
                                        width: Fill
                                        height: Fit
                                        text: ""
                                        draw_text.color: #xcacaca
                                        draw_text.text_style: theme.font_code{font_size: 12}
                                    }
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 1.0 y: 0.5}
                                    extensions_close := Button{
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
                    // ── Context management modal ──
                    context_modal := Modal{
                        content +: {
                            width: 500
                            height: 520
                            flow: Down

                            SolidView{
                                width: Fill height: Fill
                                padding: Inset{top: 14 right: 14 bottom: 14 left: 14}
                                flow: Down spacing: 10
                                draw_bg.color: #x1b1b20

                                Label{
                                    text: "Context management"
                                    draw_text.color: #xeaeaea
                                    draw_text.text_style.font_size: 14
                                }
                                ScrollYView{
                                    width: Fill height: Fill
                                    padding: Inset{right: 10}
                                    context_body := Label{
                                        width: Fill
                                        height: Fit
                                        text: ""
                                        draw_text.color: #xcacaca
                                        draw_text.text_style: theme.font_code{font_size: 12}
                                    }
                                }
                                View{
                                    width: Fill height: Fit
                                    flow: Right spacing: 8 align: Align{x: 0.0 y: 0.5}
                                    context_compact := Button{
                                        text: "Compact"
                                        draw_bg.color: #x2a2a30
                                        draw_bg.color_hover: #x3a3a40
                                        draw_bg.color_down: #x1a1a20
                                        draw_text.color: #xcacaca
                                        draw_text.text_style.font_size: 12
                                    }
                                    context_clear := Button{
                                        text: "Clear"
                                        draw_bg.color: #x5f3a00
                                        draw_bg.color_hover: #x7f5000
                                        draw_bg.color_down: #x4a2a00
                                        draw_text.color: #xeaeaea
                                        draw_text.text_style.font_size: 12
                                    }
                                    context_new_session := Button{
                                        text: "New Session"
                                        draw_bg.color: #x1a4a2a
                                        draw_bg.color_hover: #x2a6a3a
                                        draw_bg.color_down: #x0a3a1a
                                        draw_text.color: #xeaeaea
                                        draw_text.text_style.font_size: 12
                                    }
                                    View{ width: Fill height: 1 }
                                    context_close := Button{
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
                }
            }
        }
    }
}