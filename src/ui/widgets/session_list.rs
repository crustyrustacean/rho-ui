use crate::util::formatting::relative_time;
use makepad_widgets::*;
use std::sync::RwLock;

/// One row in the session picker modal.
#[derive(Clone)]
pub(crate) struct SessionEntry {
    pub path: String,
    pub mtime_secs: u64,
    pub entry_count: u64,
}

static SESSIONS: RwLock<Vec<SessionEntry>> = RwLock::new(Vec::new());

/// Replace the entire session list. Called by App after fetching from rho.
pub(crate) fn set_sessions(sessions: Vec<SessionEntry>) {
    *SESSIONS.write().unwrap() = sessions;
}

/// Read the session at `index`, if any.
pub(crate) fn get_session(index: usize) -> Option<SessionEntry> {
    SESSIONS.read().unwrap().get(index).cloned()
}

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
                    if let Some(entry) = sessions.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        // Monospace, fixed-width columns so rows line up:
                        //   date (11) | entries (12) | path (rest)
                        let row = format!(
                            "{:<11}{:<12}{}",
                            relative_time(entry.mtime_secs),
                            format!("{} entries", entry.entry_count),
                            entry.path
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
