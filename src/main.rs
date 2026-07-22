pub use makepad_widgets;
use crate::makepad_widgets::*;

mod agent;
mod app;
mod chat;
mod ui;
mod util;

pub use app::App;

// ── Diagnostic tracing ─────────────────────────────────────────────────────
// Silent by default; enable with `RHO_UI_TRACE=1` to see every `RhoEvent` and
// `busy` transition. Kept here (not in `app.rs`) so `trace!` is crate-rooted
// and trivially reachable from any module via `crate::trace!`.

pub(crate) fn trace_enabled() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("RHO_UI_TRACE")
            .map(|v| !v.is_empty() && v != "0")
            .unwrap_or(false)
    })
}

#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::trace_enabled() {
            eprintln!($($arg)*);
        }
    };
}

app_main!(App);