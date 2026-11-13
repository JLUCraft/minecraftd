# minecraftd

A composable Rust library for Minecraft instance management, downloads, mod loaders, world archives, game startup, and server queries.

## Setup and structure

Public APIs live under `src/`; integration tests live under `tests/`. Sibling `federated-server` and `FollyLauncher` consume this library through path dependencies.

Download plans use `download::request::DownloadParam` for ordered source URLs, destination, integrity metadata, and optional file size. Loader preparation returns bound processor steps with its downloads so the host can persist and execute the installation plan. Resource source selection is defined in `resource::mirrors`.

Process spawns accept the initial output callback before readers start. Output uses a bounded queue and segments of at most 8 KiB. Ring buffers split long lines into bounded segments and render invalid UTF-8 with replacement characters. `OutputRingBuffer::recent_with_offset` and `InstanceService::recent_output_with_offset` return the global first-segment index and captured text. The counter includes evicted segments, is shared by clones, and survives clearing.


`ServerConfig.server_type` controls Java startup flags; universal services do not receive `--nogui`. On Unix, an empty stop command sends SIGTERM, and `^C` sends SIGINT. Stop waits for process exit before releasing the instance. A forced local kill waits for its exit notification.

## Validation

Select one validation channel for a batch and reuse successful results for unchanged inputs. Standard commands:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

Read [AGENTS.md](AGENTS.md) before making changes.
