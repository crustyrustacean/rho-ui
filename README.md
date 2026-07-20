# rho-ui

`rho-ui` is a native graphical frontend for [rho](https://github.com/crustyrustacean/rho), a Rust-focused coding agent. It is built in Rust using the [Makepad](https://github.com/makepad/makepad) UI framework.

## Features

- Streamed assistant responses and reasoning
- Tool-call display with expandable details
- Interactive approval requests
- Model and provider selection
- Session listing and resumption
- Context usage and session statistics
- Makepad Studio integration

## Requirements

- A Rust toolchain
- The Makepad repository checked out next to this project at `../makepad`
- The `rho` CLI installed and available on `PATH`

## Build and run

```sh
cargo run
```

Run the checks and test suite with:

```sh
cargo check
cargo test
cargo clippy
```

## Makepad Studio

The included `makepad.splash` defines Studio run items for launching, checking, testing, and linting the application. Open this project in Makepad Studio and select the appropriate run item.

## Architecture

`rho-ui` launches the `rho` CLI as a child process and communicates with it using JSON-RPC 2.0 over standard input and output. Makepad renders the chat interface and receives asynchronous agent events through its event loop.

## License

This project is licensed under the MIT License. See [License.txt](License.txt) for details.
