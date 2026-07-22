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
  main.rs      # the whole app (~2,900 lines) — being modularized
  util/        # Phase 1: json + formatting helpers (+ co-located tests)
makepad.splash # Studio build/run hub
resources/     # app icon
.rho/          # rho project config + memory (see "rho project memory")
```

## In progress: main.rs modularization
`main.rs` currently holds everything and is being split into modules per an
**8-phase plan. Phase 1 (util extraction) is done.** The full plan and the
Makepad hot-reload assessment are saved in rho's project memory (titles:
"rho-ui main.rs modularization plan", "rho-ui Makepad Studio hot-reload
assessment"). Next phases: `app.rs`, `agent/`, `chat/`, `ui/`. **Put new code
in its target module — don't keep growing `main.rs`.**

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
- The 3 `redundant field names` clippy warnings in `main.rs` are
  **pre-existing** (not from recent work) — leave them or fix in a dedicated pass.
- `rho` is external: if the UI won't connect / nothing happens, confirm `rho`
  is built and resolvable (`RHO_PATH` or `PATH`) **before** debugging the UI.

## rho project memory
Memory is enabled (`.rho/config.toml`): project overview, Makepad architecture
reference, and a Studio integration guide are stored there. When resuming work,
read rho's memory to re-orient before editing.
