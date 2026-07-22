pub(crate) mod chat_scroll;
pub(crate) mod model_list;
pub(crate) mod provider_list;
pub(crate) mod session_list;

pub(crate) use chat_scroll::ChatScroll;
pub(crate) use model_list::ModelList;
pub(crate) use provider_list::ProviderList;
pub(crate) use session_list::SessionList;

use makepad_widgets::*;

/// Scan a PortalList (identified by `list_id`) inside the UI for any row whose
/// `pick` button was clicked this event cycle. Returns the item ids that were
/// picked. Used by the model/session provider pickers — they share the same
/// `row` / `pick` template structure.
pub(crate) fn picked_items(ui: &WidgetRef, cx: &mut Cx, actions: &Actions, list_id: &[LiveId]) -> Vec<usize> {
    let mut picked = Vec::new();
    let list = ui.widget(cx, list_id).portal_list(cx, ids!(list));
    for (item_id, item) in list.items_with_actions(actions) {
        if item.button(cx, ids!(pick)).clicked(actions) {
            picked.push(item_id);
        }
    }
    picked
}
