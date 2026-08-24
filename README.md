# Glasshouse

A lean, project-scoped control plane for existing coding-agent harnesses such as
Claude Code, Codex, and Antigravity.

Glasshouse does not replace those products or hide them behind a proprietary
agent loop. It starts and manages **real native harness sessions**, keeps every
session directly observable and interactive, routes work between sessions and
available model resources, records project-specific knowledge, and lets an
orchestrator session delegate work to other first-class sessions.

> Glasshouse orchestrates agents without hiding them.

## Status

Under active implementation. `GLASSHOUSE_IMPLEMENTATION_CAPABILITY_MAP.md` is
the authoritative specification and tracks what is done.

## Build

```sh
cargo build --release
```

The result is a single `glasshouse` executable with no daemon, background
service, Node, or Python requirement.

## Usage

```sh
glasshouse                    # operate on the current project
glasshouse --scope <path>     # select a project root explicitly
glasshouse --help
```

Glasshouse operates on exactly one project root — the containing Git repository
when there is one, otherwise the current directory. All state, sessions, and
memory are isolated per project root.

### Environment

| Variable | Purpose |
| --- | --- |
| `GLASSHOUSE_DATA_DIR` | Override the per-user application-data directory |
| `GLASSHOUSE_CONFIG_DIR` | Override the per-user configuration directory |
| `GLASSHOUSE_LOG` | Enable logging with a tracing filter, e.g. `debug` |

## License

MIT OR Apache-2.0
