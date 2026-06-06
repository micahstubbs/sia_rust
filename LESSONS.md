# LESSONS.md

Append-only debugging and process lessons for this project.

## 2026-06-06T20:30 - Install Rust CLIs with the repo lockfile and intended toolchain

**Problem**: Reinstalling the local `beads_rust` binary with plain `cargo install --path ...` failed even though a working older binary was already installed.

**Root Cause**: The first install used stable Rust, but the `fsqlite` dependency graph requires nightly-only features. Retrying with nightly but without `--locked` allowed Cargo to float transitive dependencies; `fsqlite-core 0.1.7` then compiled against an incompatible `asupersync 0.3.2` API instead of the lockfile-pinned `asupersync 0.3.1`.

**Lesson**: For local Rust CLI installs from a checked-out repository, verify the active binary, source version, toolchain, and lockfile expectations before reinstalling. Use the repository's lockfile when the checkout has one.

**Solution**: Reinstalled with `cargo +nightly install --locked --path <checkout> --root /home/m/.local --force`, then verified `command -v`, `--version`, workspace health, and smoke-test behavior.

**Prevention**: Prefer `cargo +<toolchain> install --locked --path ... --root ... --force` for project-local CLI installs. If a plain install fails in a dependency, compare the lockfile-pinned versions against the floated versions before changing source code.
