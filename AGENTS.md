# Working in op-cache

A Rust CLI that fronts the 1Password CLI with an in-memory cache. One binary
plays two roles: the client people call, and the daemon it starts on first use.
`CLAUDE.md` only imports this file.

Rules from `~/agentic-code/AGENTS.md` (use `guise-gh`, never touch git config or
remotes) still apply. What follows is specific to this repo.

## Layout

```text
src/main.rs      the CLI: read, run, config, status, inspect, clear, stop, and passthrough to op
src/client.rs    connects to the daemon, spawning it when nothing answers
src/daemon.rs    serves the cache over a unix socket; one JSON line each way
src/cache.rs     the store, with expiry
src/protocol.rs  the request and response shapes
src/config.rs    ~/.config/op-cache/config.toml, socket and config paths
src/wizard.rs    the interactive `op-cache config`
src/prompt.rs    the wizard's clack-style prompts: select, input, confirm
src/op.rs        running the real op
tests/cli.rs     end-to-end, against a stub op
```

## Tests

```bash
cargo test                        # unit + end-to-end, no network, no real op
docker compose run --rm tests     # the same, on Linux
```

CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` on macOS and Linux; all three are the merge gate.

`tests/cli.rs` sandboxes every scenario: its own config file, its own socket
via `OP_CACHE_SOCKET`, a cleared environment, and a stub `op` that logs each
call. Assert on what the stub saw and what the daemon reports, not on exit
codes alone. Never point a test at the real `op`: it would prompt for the
human's vault and cache their secrets.

The daemon a test starts is detached from the test process. The harness stops
it on drop; keep that working when adding scenarios.

## Behavior worth preserving

- Anything op-cache doesn't recognize goes to `op` unchanged: unknown
  subcommands, leading flags, `run` with flags before `--`, `read -o`.
- The cache key for `read` is the full argument list, so `--account` and
  friends never cross-contaminate.
- A reference's lifetime is the most specific `[overrides]` match, where a
  key ending in `/` covers everything under it, otherwise the global `ttl`.
- A failed `op read` is never cached and its exit code is passed through.
- `run` fetches every miss behind one `op run`: one prompt, however many
  references the environment holds.
- With no daemon reachable, `read` and `run` still work; they just call `op`.
- The socket is mode 600 and lives in a per-user directory. Secrets never
  touch disk and never appear in argv.

## Commits and PRs

One line, imperative. One commit per logical change. Stage what you changed,
never `git add -A`. PR descriptions are short and name the model and harness
at the end.
