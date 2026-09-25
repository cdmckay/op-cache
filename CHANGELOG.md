# Changelog

Notable changes to this fork of
[PatrickTulskie/op-cache](https://github.com/PatrickTulskie/op-cache). The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions
follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

To be released as 0.2.0.

### Changed

From upstream, after its 0.1.0:

- The README recommends installing from the Homebrew tap.
- The daemon locks its socket with std instead of the `fs2` crate.
- `op-cache config` draws its own prompts instead of using `cliclack`.
- The command line is parsed by hand instead of with `clap`. Per-command help,
  argument errors, stray words after `help`, and misuse of `daemon` no longer
  reach `op`.

In this fork:

- A cached secret lives at most a day. A longer `ttl`, override, or requested
  lifetime is capped; `until-exit` is not a duration and is unaffected.

### Fixed

- A lifetime too long to add to the clock no longer panics the daemon (it is
  capped at a day instead). The panic happened while the daemon held its lock,
  so every later request panicked too, `op-cache stop` and the idle timeout
  among them, leaving a daemon that held its secrets until it was killed. A
  poisoned lock now shuts the daemon down instead.

## [0.1.0] - 2026-09-19

Upstream's first release, and this fork's starting point.

[Unreleased]: https://github.com/cdmckay/op-cache/compare/a852de2...main
[0.1.0]: https://github.com/PatrickTulskie/op-cache/releases/tag/v0.1.0
