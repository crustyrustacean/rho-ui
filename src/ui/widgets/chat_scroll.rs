use crate::chat::{ChatBlock, ToolStatus, ApprovalResolution};
use crate::util::formatting::{cap_head, cap_tail};
use makepad_widgets::*;

#[derive(Script, ScriptHook, Widget)]
pub struct ChatScroll {
    #[deref]
    view: View,
}

impl Widget for ChatScroll {
    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        // Snapshot the blocks so we don't hold the lock across the draw.
        let blocks = crate::chat::store::snapshot();

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
                            ChatBlock::Response { text, expanded } => {
                                let w = list.item(cx, item_id, id!(Response));
                                let display = if *expanded {
                                    text.clone()
                                } else {
                                    cap_head(text, 4000)
                                };
                                w.markdown(cx, ids!(msg)).set_text(cx, &display);
                                // Show expand/collapse when text is long enough to have been truncated.
                                let can_toggle = text.chars().count() > 4000;
                                w.widget(cx, ids!(expand_btn)).set_visible(cx, can_toggle);
                                if can_toggle {
                                    w.button(cx, ids!(expand_btn)).set_text(cx, if *expanded { "Collapse" } else { "Expand" });
                                }
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