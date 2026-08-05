# rho-ui AGENTS.md

## What this is
`rho-ui` is a native desktop GUI (built with [Makepad](https://makepad.nl)) that
fronts the **`rho`** coding agent. It spawns a `rho` process in headless
JSON-RPC mode and renders a chat over its stdin/stdout. The `rho` binary lives
in a **separate repo** (`rho-coding-agent`); this crate only launches it.

## Stack
- Rust, single crate (edition 2021).
- `makepad-widgets` via a **path dependency** (`../makepad/widgets`) — the
  sibling `makepad` repo must be checked out next to this one.
- `serde_json` for the JSON-RPC conversation.

## Build / test / lint / run
```sh
cargo check  -p rho-ui
cargo test   -p rho-ui     # ~67 unit tests
cargo clippy -p rho-ui
cargo run    -p rho-ui
```
These same tasks are wired into `makepad.splash` (the Studio hub): `rho-ui`,
`Check`, `Test`, `Clippy`.

## Running it
- **Makepad Studio** is the primary dev loop — open the repo and run `rho-ui`.
  See the hot-reload gotcha below for what does/doesn't reload.
- `rho` is launched via the `RHO_PATH` env var, else `rho` from `PATH`. Build
  and install `rho` from the `rho-coding-agent` repo first
  (`~/.local/bin/rho`).

## Project structure
```
src/
  main.rs           # entry point only: module decls, trace! macro, app_main!(App)
  app.rs            # App struct, event handling, rho-event dispatch, spinner, usage
  agent/
    mod.rs          #   re-exports RhoAgent, RhoEvent, RequestKind
    process.rs      #   RhoAgent: subprocess spawn, mpsc drain, JSON-RPC write + RPC wrappers
    protocol.rs     #   RhoEvent enum, RequestKind, parse_notification + tests
  chat/
    mod.rs          #   re-exports ChatBlock, ToolStatus, ApprovalResolution
    model.rs        #   ChatBlock enum and friends
    store.rs        #   global RwLock<Vec<ChatBlock>> + push/append/finalize/resolve ops + tests
  ui/
    mod.rs          #   module wiring + re-exports
    script.rs       #   the entire script_mod! DSL (window, bars, widget templates, modals)
    widgets/
      mod.rs        #     ChatScroll/ModelList/SessionList/ProviderList + picked_items helper
      chat_scroll.rs#     PortalList draw per ChatBlock variant
      model_list.rs #     RwLock-backed model row data + Widget
      session_list.rs    RwLock-backed session row data + Widget
      provider_list.rs   RwLock-backed provider row data + Widget
  util/
    mod.rs          #   module wiring
    json.rs         #   jstr/ju64/jf64/jbool helpers + tests
    formatting.rs   #   format_secs, relative_time, cap_head/tail, session stats + tests
makepad.splash      # Studio build/run hub
resources/          # app icon
.rho/               # rho project config + memory (see "rho project memory")
```

## Modularization: complete
The 8-phase plan to split a monolithic `main.rs` (~2,900 lines) into modules
is **done**. `main.rs` is now ~35 lines (entry point only). Every feature
lives in its target module — **put new code in the right module, never grow
`main.rs`.** Key architectural consequence: chat state is a single global
`RwLock<Vec<ChatBlock>>` (`chat::store::CHAT_BLOCKS`); `App` mutates it and
tells the PortalList to tail + redraw. The picker widgets (model/session/
provider) each hold their own `RwLock` static for row data, written by `App`
and read during `Widget::draw_walk`.

## Conventions
- **Commits go straight to `trunk`** (solo project; no PR/branch flow).
- Conventional-ish messages: `refactor:`, `fix(ui):`, `docs:`, `chore:`.
- Prefer `pub(crate)` for internal helpers (see `src/util/`).
- Co-locate unit tests with the code they test.

## ⚠️ Makepad Studio hot-reload (the big one)
- **UI / `script_mod!` / live-design changes hot-reload** in Studio.
- **Rust logic changes do NOT** — they require a recompile and process restart.
- If an edit "didn't take," **rebuild first** before assuming the code is wrong.

## Gotchas
- `makepad-widgets` is a **path dep** → changes in the sibling `makepad` repo
  affect this build, and `../makepad` must be present.
- `cargo clippy` is currently **clean** (zero warnings).
- `rho` is external: if the UI won't connect / nothing happens, confirm `rho`
  is built and resolvable (`RHO_PATH` or `PATH`) **before** debugging the UI.

## rho project memory
Memory is enabled (`.rho/config.toml`): project overview, Makepad architecture
reference, and a Studio integration guide are stored there. When resuming work,
read rho's memory to re-orient before editing.