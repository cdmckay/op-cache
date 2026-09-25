# op-cache

A read-through, in-memory cache in front of the 1Password CLI.

Agents and command line tools that pull credentials from 1Password get
interrupted every time the vault locks: another biometric prompt, another
stalled command, or an agent that can't continue because nobody was there to
approve it. op-cache ties those secrets to your login session instead. Sign in,
warm the cache with the first call, and everything after that is served from
memory for as long as you're logged in, whether or not 1Password has locked
since.

`op-cache read` asks a small background daemon first and only calls `op` when
the daemon doesn't have the answer. The first call in a session starts the
daemon; every call after that comes back in milliseconds.

```bash
$ time op-cache read op://secrets/GITHUB_TOKEN/credential   # calls op
real    0m7.636s
$ time op-cache read op://secrets/GITHUB_TOKEN/credential   # from memory
real    0m0.015s
```

## Install

op-cache needs the [1Password CLI](https://developer.1password.com/docs/cli/)
on your `PATH` as `op`.

<details open>
<summary><b>Homebrew</b> (macOS and Linux)</summary>

```bash
brew install PatrickTulskie/tap/op-cache
```

</details>

<details>
<summary><b>Prebuilt binary</b></summary>

Every [release](https://github.com/PatrickTulskie/op-cache/releases) has a
tarball for macOS and Linux on arm64 and x86_64, plus a `SHA256SUMS` file.
Unpack `op-cache` somewhere on your `PATH`.

</details>

<details>
<summary><b>From source</b></summary>

```bash
cargo install --git https://github.com/PatrickTulskie/op-cache
```

</details>

After upgrading, run `op-cache stop` once so the next call starts a daemon from
the new build.

## Use

Export secret references the way you already do for `op run`, then swap `op`
for `op-cache`:

```bash
export GITHUB_TOKEN=op://secrets/GITHUB_TOKEN/credential
export DATABASE_URL=op://secrets/DATABASE_URL/url

alias db="op-cache run -- psql"
token=$(op-cache read $GITHUB_TOKEN)
```

| Command | What it does |
|---|---|
| `op-cache read <ref> [op flags]` | The secret, from memory when possible. On a miss it runs `op read` with the same arguments and remembers the answer. |
| `op-cache run -- <command>` | Runs the command with every `op://` value in the environment resolved, the way `op run` does. |
| `op-cache config` | Interactive setup of everything below. |
| `op-cache status` | Whether the daemon is up and how it's configured. |
| `op-cache inspect` | Every reference in memory, a masked peek at its value, and when it expires. |
| `op-cache clear` | Forget every secret, keep the daemon. |
| `op-cache stop` | Stop the daemon, which forgets everything. |
| anything else | Handed to `op` unchanged, so `op-cache item list` is just `op item list`. |

`op-cache read` and `op-cache run` share one cache. Flags matter: `read
--account work <ref>` is a different entry from `read <ref>`. If you pass a
flag that op-cache doesn't handle itself, such as `run --env-file` or `read
-o`, the whole command goes to `op` and nothing is cached.

Unlike `op run`, `op-cache run` does not mask secrets that the command prints.
It execs the command directly so terminals and interactive programs behave
normally.

## Configuration

`op-cache config` walks through the settings and writes
`~/.config/op-cache/config.toml`. The file is small enough to edit by hand:

```toml
ttl = "until-exit"      # or a duration: "30m", "2h", "1d"
idle_timeout = "never"  # or a duration; the daemon exits after that long with no requests
op = "op"               # the binary to call
# socket = "/some/where/op-cache.sock"
```

Out of the box a secret stays cached until the daemon exits, and the daemon
runs until you `op-cache stop` or reboot. Set `ttl` to re-read from 1Password
on a schedule, and `idle_timeout` to have the daemon shut itself down and drop
everything after a quiet stretch. A duration is capped at a day, here and in
`[overrides]`; `until-exit` is not a duration and isn't capped.

Individual references can have their own lifetime, and a key ending in `/`
covers everything in that vault or item. The most specific match wins:

```toml
ttl = "until-exit"

[overrides]
"op://secrets/" = "1h"
"op://secrets/DEPLOY_KEY/credential" = "5m"
```

The wizard offers whatever the daemon currently holds when you add an
override, so run your usual commands first and pick from the list.
`op-cache inspect` shows how long each cached entry has left.

The daemon reads `idle_timeout` and `socket` when it starts, so change those
and then `op-cache stop`; the next call starts a fresh one. `ttl`, `overrides`
and `op` apply immediately.

`OP_CACHE_CONFIG` and `OP_CACHE_SOCKET` override the config and socket paths
for one invocation.

## How it works

- The daemon is the same binary, run as `op-cache daemon` by the first client
  that finds nothing listening. It detaches from the terminal and serves a
  unix socket, mode 600, in `$XDG_RUNTIME_DIR` or your `$TMPDIR`.
- The client does the fetching. On a miss it runs `op read` with your terminal
  attached, so a locked vault prompts you the same way `op` always has, then
  hands the result to the daemon. The daemon never talks to 1Password itself.
- A failed `op read` is not cached, and its exit code and stderr pass through.
- If the daemon can't be started, `read` and `run` fall back to plain `op`.
- Secrets never touch disk and never appear on a command line. `inspect` masks
  values inside the daemon before they cross the socket.

## Development

```bash
cargo test                        # unit and end-to-end tests, no real op involved
docker compose run --rm tests     # the same suite on Linux
```

Tagging `v*` builds macOS and Linux binaries and attaches them to a GitHub
release.
