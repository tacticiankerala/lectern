# Lectern

A native Markdown reader for Windows, in Rust, Tauri 2 and plain TypeScript. These notes are for coding agents and contributors alike.

## Invented data only

This repository is public. Everything in it that stands for user data is invented: test data, fixtures, examples, snapshots, screenshots and commit messages.

- Never derive names, titles, folder names, codenames or paths from a real machine, a real notes vault, or any user's files, your own included. Make them up.
- Use these stand-ins: `S:\Notes\My Vault` for a vault, `\\nas\share` for a network share, `/home/me` for a Linux home and `C:\Users\me` for a Windows profile.
- Screenshots show only the fictional library in `fixtures/demo`.
- The `privacy` job in `.github/workflows/ci.yml` fails on any tracked text file that holds a real-looking user-profile path. It checks paths only; keeping names, titles and codenames invented is up to you.

## Layout

- `crates/lectern-core`: rendering, library, search and settings. Pure Rust; builds and tests on any platform.
- `src-tauri`: the Windows app around it.
- `ui`: the interface, TypeScript and CSS with no framework. The core tests rewrite the ts-rs bindings in `ui/src/generated`.
- `fixtures/vault` is the library the tests run against; `fixtures/demo` is the one in the screenshots.

## Commands

```bash
cargo fmt --all -- --check
cargo clippy -p lectern-core --all-targets -- -D warnings
cargo test -p lectern-core
cd ui && npm run typecheck && npm run lint && npm test && npm run e2e
```

To build the Windows app from WSL, see "From WSL, cross-compiling" in the README.
