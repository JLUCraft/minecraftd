# minecraftd

A composable Rust library for Minecraft instance management, downloads, mod loaders, world archives, game startup, and server queries.

## Setup and structure

Public APIs live under `src/`; integration tests live under `tests/`. Sibling `federated-server` and `FollyLauncher` consume this library through path dependencies.

## Validation

Select one validation channel for a batch and reuse successful results for unchanged inputs. Standard commands:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

Read [AGENTS.md](AGENTS.md) before making changes.
