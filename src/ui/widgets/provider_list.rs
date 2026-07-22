use makepad_widgets::*;
use std::sync::RwLock;

/// One row in the provider picker modal.
#[derive(Clone)]
pub(crate) struct ProviderEntry {
    pub name: String,
    pub reachable: bool,
    pub active: bool,
    pub is_external: bool,
}

static PROVIDERS: RwLock<Vec<ProviderEntry>> = RwLock::new(Vec::new());

/// Replace the entire provider list. Called by App after fetching from rho.
pub(crate) fn set_providers(providers: Vec<ProviderEntry>) {
    *PROVIDERS.write().unwrap() = providers;
}

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
                    if let Some(entry) = providers.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        let state = if entry.reachable { "\u{2713}" } else { "\u{2717}" };
                        let flags = match (entry.active, entry.is_external) {
                            (true, true) => " [active, external]",
                            (true, false) => " [active]",
                            (false, true) => " [external]",
                            (false, false) => "",
                        };
                        w.button(cx, ids!(pick)).set_text(cx, &format!("{} {}{}", entry.name, state, flags));
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
