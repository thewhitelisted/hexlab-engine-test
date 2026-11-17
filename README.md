# hexlab engine

A deterministic rules engine for a Catan-like board game, written in Rust.

- `engine/core`: board topology, game state, legal moves, rule checks, belief tracking over
  hidden hands, and game records that replay exactly. How to use it: `docs/ENGINE.md`.
- `engine/wasm`: the engine compiled to WebAssembly for use in a browser.
- `docs/RULES.md`: the rules the engine implements, with the choices it makes where the rulebook
  is silent. `docs/TRACE_FORMAT.md`: the game export that other engines replay.

Build and test with `cargo test --workspace --locked` (Rust with edition 2024). The browser
build is `cargo build --profile wasm --locked -p engine-wasm --target wasm32-unknown-unknown`.
CI (`.github/workflows/ci.yml`) runs the format check, Clippy, the tests in debug and release,
and the browser build.

This repository is published from a private one, which holds the history; each commit here is a
copy of the engine as it stands there. Issues are welcome; changes are made in the private
repository and arrive here with the next sync.

MIT licence (`LICENSE`).
