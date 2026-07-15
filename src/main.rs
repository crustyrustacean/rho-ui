pub use makepad_widgets;

use makepad_widgets::*;

app_main!(App);

// ── The UI, in Script DSL ───────────────────────────────────────────────────
// Layout (top → bottom):
//   1. Title bar (Fit) — "rho" branding + subtitle.
//   2. Menu bar (Fit) — row of buttons: Session, Model, Resume, Providers,
//      Abort, Help, Quit. Replaces the TUI's slash commands + Ctrl-key shortcuts.
//   3. Output scrollback (Fill) — user blocks, reasoning, responses,
//      tool-call blocks with status-colored backgrounds.
//   4. Working line (Fit) — spinner + "Working" + elapsed + activity.
//   5. Input box (Fit, 2 lines) — bordered multi-line text input.
//   6. Footer (Fit, 2 lines) — cwd + git branch, then token/cost/context stats
//      (left) + model name (right).
//
// All content is static placeholder text — no state, no event handlers yet.
script_mod! {
    use mod.prelude.widgets.*

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
                    scrollback := View{
                        width: Fill height: Fill
                        padding: Inset{top: 8, right: 12, bottom: 8, left: 12}
                        flow: Down spacing: 6
                        scroll_bars: ScrollBars{
                            show_scroll_x: false
                            show_scroll_y: true
                        }

                        // ── User message block (bg 239 = #x303045) ──
                        user_block := SolidView{
                            width: Fill height: Fit
                            padding: 8
                            draw_bg.color: #x303045
                            user_label := Label{
                                width: Fill
                                text: "Review this project and gain context on it."
                                draw_text.color: #xdcdcdc
                                draw_text.text_style.font_size: 13
                            }
                        }

                        spacer2 := View{ width: Fill height: 4 }

                        // ── Reasoning block (collapsed, done) ──
                        reasoning_header := Label{
                            width: Fill
                            text: "? thought · 2.3s"
                            draw_text.color: #x7a7a7a
                            draw_text.text_style: theme.font_italic{
                                font_size: 12
                            }
                        }
                        reasoning_tail := Label{
                            width: Fill
                            text: "Let me explore the project structure and read the key files to understand what's going on."
                            draw_text.color: #x7a7a7a
                            draw_text.text_style: theme.font_italic{
                                font_size: 12
                            }
                        }

                        spacer3 := View{ width: Fill height: 4 }

                        // ── Response text (markdown) ──
                        response := Label{
                            width: Fill
                            text: "I've reviewed the project. It's a GUI frontend built with makepad, using the Script DSL. The layout is a three-region chat-style window with a header, middle content area, and footer."
                            draw_text.color: #xdcdcdc
                            draw_text.text_style.font_size: 13
                        }

                        spacer4 := View{ width: Fill height: 8 }

                        // ── Tool call block: success (bg 22 = #x005500) ──
                        tool_success := SolidView{
                            width: Fill height: Fit
                            padding: 8
                            flow: Down spacing: 4
                            draw_bg.color: #x00551a
                            tool_success_head := Label{
                                width: Fill
                                text: "read_file  src/main.rs  ✓ done"
                                draw_text.color: #xeaeaea
                                draw_text.text_style: theme.font_bold{
                                    font_size: 13
                                }
                            }
                            tool_success_out := Label{
                                width: Fill
                                text: "pub use makepad_widgets;\nuse makepad_widgets::*;\n\napp_main!(App);"
                                draw_text.color: #x9a9a9a
                                draw_text.text_style.font_size: 12
                            }
                        }

                        spacer5 := View{ width: Fill height: 4 }

                        // ── Tool call block: pending (bg 238 = #x2e2e2e) ──
                        tool_pending := SolidView{
                            width: Fill height: Fit
                            padding: 8
                            flow: Down spacing: 4
                            draw_bg.color: #x2e2e2e
                            tool_pending_head := Label{
                                width: Fill
                                text: "cargo_check  ⟳ running"
                                draw_text.color: #xeaeaea
                                draw_text.text_style: theme.font_bold{
                                    font_size: 13
                                }
                            }
                        }

                        spacer6 := View{ width: Fill height: 4 }

                        // ── Tool call block: error (bg 52 = #x5f0000) ──
                        tool_error := SolidView{
                            width: Fill height: Fit
                            padding: 8
                            flow: Down spacing: 4
                            draw_bg.color: #x5f1a1a
                            tool_error_head := Label{
                                width: Fill
                                text: "edit_file  src/main.rs  ✗ failed"
                                draw_text.color: #xeaeaea
                                draw_text.text_style: theme.font_bold{
                                    font_size: 13
                                }
                            }
                            tool_error_out := Label{
                                width: Fill
                                text: "error: hash mismatch at line 42"
                                draw_text.color: #x9a9a9a
                                draw_text.text_style.font_size: 12
                            }
                        }

                        spacer7 := View{ width: Fill height: 8 }

                        // ── Second user message ──
                        user_block2 := SolidView{
                            width: Fill height: Fit
                            padding: 8
                            draw_bg.color: #x303045
                            user_label2 := Label{
                                width: Fill
                                text: "Can you fix the compilation error?"
                                draw_text.color: #xdcdcdc
                                draw_text.text_style.font_size: 13
                            }
                        }

                        spacer8 := View{ width: Fill height: 4 }

                        // ── Live reasoning (streaming) ──
                        reasoning_streaming := Label{
                            width: Fill
                            text: "? thinking"
                            draw_text.color: #x7a7a7a
                            draw_text.text_style: theme.font_italic{
                                font_size: 12
                            }
                        }
                        reasoning_streaming_tail := Label{
                            width: Fill
                            text: "The hash mismatch is because the file changed since it was last read. I need to re-read the file to get fresh hashes, then retry the edit."
                            draw_text.color: #x7a7a7a
                            draw_text.text_style: theme.font_italic{
                                font_size: 12
                            }
                        }
                    }

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
// Minimal for now: just holds a handle to the UI tree and forwards OS events
// into it. No state, no handlers — those arrive when we wire rho's events in.
#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, _cx: &mut Cx, _actions: &Actions) {}
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}