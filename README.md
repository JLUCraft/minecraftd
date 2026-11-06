# minecraftd

A composable Rust library for Minecraft instance management, downloads, mod loaders, world archives, game startup, and server queries.

## Setup and structure

Public APIs live under `src/`; integration tests live under `tests/`. Sibling `federated-server` and `FollyLauncher` consume this library through path dependencies.

`OutputRingBuffer::recent_with_offset` and `InstanceService::recent_output_with_offset` return a window's zero-based global first-line index and its lines. The index counts all written lines, including evicted lines, and is shared by buffer clones. Empty windows return the next write index. Clearing the buffer preserves the counter; a new buffer starts at zero. Existing line-only queries remain available.

## Validation

Select one validation channel for a batch and reuse successful results for unchanged inputs. Standard commands:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

Read [AGENTS.md](AGENTS.md) before making changes.
