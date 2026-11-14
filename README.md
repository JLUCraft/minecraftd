# minecraftd

A composable Rust library for Minecraft instance management, downloads, mod loaders, world archives, game startup, and server queries.

## Setup and structure

Public APIs live under `src/`; integration tests live under `tests/`. Sibling `union-server` and `FollyLauncher` consume this library through path dependencies.

Download plans use `download::request::DownloadParam` for ordered source URLs, destination, integrity metadata, and optional file size. Loader preparation returns bound processor steps with its downloads so the host can persist and execute the installation plan. Resource source selection is defined in `resource::mirrors`. Forge and NeoForge server installation uses the shared native processor plan with server mappings and server-only steps. Download queues retain ownership of their tasks and abort pending transfers when dropped.

Process spawns accept the initial output callback before readers start. Output uses a bounded queue and segments of at most 8 KiB. Ring buffers split long lines into bounded segments and render invalid UTF-8 with replacement characters. `OutputRingBuffer::recent_with_offset` and `InstanceService::recent_output_with_offset` return the global first-segment index and captured text. The counter includes evicted segments, is shared by clones, and survives clearing.


`ServerConfig.server_type` controls Java startup flags; universal services do not receive `--nogui`. On Unix, an empty stop command sends SIGTERM, and `^C` sends SIGINT. Stop waits for process exit before releasing the instance. A forced local kill waits for its exit notification. `InstanceService.stop_gracefully` reports the observed exit status and retains ownership when its deadline fails. Native supervisors use `LocalSpawner::with_group_observation` to adopt and wait for Linux descendant exits. A graceful stop preserves unresolved descendants until they exit or an owner requests force termination.

Archive managers use instance-bound IDs and publish each archive directory after
writing its data and metadata. `ArchiveRequest::new` allocates the identity so a
host can reserve ownership before publication. `ArchiveLimits` bounds source, compressed and
expanded bytes and entry counts. Retention enforces per-instance count and byte
limits. Source links and special files fail creation; extraction rejects links,
special entries, and paths outside the destination. Missing selected source paths
fail creation. ZIP and tar.gz preserve empty directories; tar.gz restores file
permissions. Restore into a caller-owned staging directory before replacing live
data. Hold exclusive ownership of the archive store across manager processes.

## Validation

Select one validation channel for a batch and reuse successful results for unchanged inputs. Standard commands:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --all-targets --locked
```

Installer and live download integration tests require network access and Java 21.
Missing prerequisites and API/download failures fail the tests. CurseForge tests
use the existing resource transport with local fixtures and require no API key.

Read [AGENTS.md](AGENTS.md) before making changes.
