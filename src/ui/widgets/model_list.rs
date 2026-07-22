use makepad_widgets::*;
use std::sync::RwLock;

/// One row in the model picker modal.
#[derive(Clone)]
pub(crate) struct ModelEntry {
    pub id: String,
    pub provider: String,
    pub is_current: bool,
}

static MODELS: RwLock<Vec<ModelEntry>> = RwLock::new(Vec::new());

/// Replace the model list and mark which one is current. Called by App after
/// filtering — the widget reads it back during draw.
pub(crate) fn set_models(models: Vec<ModelEntry>) {
    *MODELS.write().unwrap() = models;
}

/// Read the model at `index`, if any.
pub(crate) fn get_model(index: usize) -> Option<ModelEntry> {
    MODELS.read().unwrap().get(index).cloned()
}

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
                    if let Some(entry) = models.get(item_id) {
                        let w = list.item(cx, item_id, id!(row));
                        // Prefix with the provider so models from different
                        // providers (e.g. openrouter vs zai) are clearly
                        // differentiated; the list is already sorted by provider.
                        let label = if entry.is_current {
                            format!("\u{2713} {} \u{00b7} {}", entry.provider, entry.id)
                        } else {
                            format!("{} \u{00b7} {}", entry.provider, entry.id)
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
