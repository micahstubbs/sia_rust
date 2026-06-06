//! Capability allow-list for native tool execution (issue #67).
//!
//! SIA is a *self-modifying* agent framework: the native Claude runner exposes
//! `Bash`/`Read`/`Write`/`Edit`/`Glob` executors (see [`crate::llm::tools`]) that
//! act on the host filesystem. Those executors today rely on a purely *lexical*
//! path sandbox ([`resolve_in_sandbox`](crate::llm::tools)) that rejects `..` and
//! absolute escapes but does not resolve symlinks, cap file sizes, or gate which
//! shell commands may run. This module adds an explicit **capability allow-list**
//! that a runner/tool layer can consult *before* performing an action.
//!
//! The design goal is a single, auditable enforcement point that is:
//! - **pure `std`** — no new mandatory dependencies, compiled on the default build
//!   (this module is *not* gated behind the `llm` feature);
//! - **deny-by-default** — [`Capabilities::default`] grants read/write only within
//!   a declared `fs_root`, gates `Bash`, and denies network;
//! - **honest about its layer** — this is *advisory, in-process* enforcement. It
//!   raises the bar for prompt-injection-driven tool abuse and accidental escape,
//!   but it is not an OS-level sandbox. A compromised process can still bypass it.
//!   See [the roadmap](#os-level-enforcement-roadmap) below and `SECURITY.md`.
//!
//! # Usage
//!
//! ```
//! use sia::sandbox::Capabilities;
//! use std::path::PathBuf;
//!
//! let caps = Capabilities::read_only(PathBuf::from("/work"));
//! // A runner checks the capability before touching the filesystem:
//! assert!(caps.check_read("src/main.rs").is_ok());
//! assert!(caps.check_write("src/main.rs").is_err()); // read-only preset
//! assert!(caps.check_within_root("../etc/passwd").is_err()); // escape rejected
//! ```
//!
//! # OS-level enforcement roadmap
//!
//! This capability layer is the first of three hardening stages. The next stages
//! provide *kernel-enforced* isolation so that a bug or injection in the runner
//! cannot simply ignore the allow-list:
//!
//! 1. **Capability allow-list (this module).** Pure-std, advisory, in-process.
//! 2. **OS sandboxing for native execution.** On Linux, apply a
//!    [`landlock`](https://crates.io/crates/landlock) filesystem ruleset scoped to
//!    `fs_root` (unprivileged, per-thread) and a `seccomp` syscall filter (e.g.
//!    via [`seccompiler`](https://crates.io/crates/seccompiler)) to block raw
//!    network syscalls when `allow_network` is false. These give kernel-enforced
//!    confinement that survives a logic bug in the tool layer. A thin landlock
//!    integration is sketched (as a code comment below, behind a would-be
//!    `landlock-sandbox` cargo feature); it is left as roadmap rather than wired
//!    in because the `landlock` crate is not available in this build's offline
//!    dependency cache.
//! 3. **WASI component model.** Run untrusted generated agents as WebAssembly
//!    components under [`wasmtime`](https://crates.io/crates/wasmtime) with
//!    [`wasi`](https://crates.io/crates/wasi) preview2 capabilities, granting only
//!    explicit preopened directories and no ambient network/process authority.
//!
//! Each stage is additive: the capability allow-list remains the policy source of
//! truth, and stages 2/3 enforce that policy at the OS/VM boundary.

use std::error::Error;
use std::fmt;
use std::path::{Component, Path, PathBuf};

/// Default per-file byte cap for the [`Capabilities::agent_default`] profile
/// (10 MiB). This is a `const` rather than a `Config` field to keep the
/// capability layer dependency-free and the config/parity surface unchanged; a
/// stricter profile can lower it by setting `max_file_bytes` directly.
pub const AGENT_DEFAULT_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// A declarative allow-list of what a native tool executor may do.
///
/// Construct via [`Capabilities::default`] (deny-by-default within `fs_root`),
/// [`Capabilities::permissive`], or [`Capabilities::read_only`], then adjust
/// individual flags as needed. The `check_*` methods are the enforcement points.
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// Whether `Read`/`Glob` style read operations are permitted.
    pub allow_read: bool,
    /// Whether `Write`/`Edit` style mutations are permitted.
    pub allow_write: bool,
    /// Whether the `Bash` executor may run at all.
    pub allow_bash: bool,
    /// Whether outbound network access is permitted (advisory; informs the
    /// roadmap seccomp/WASI layers — this layer does not itself open sockets).
    pub allow_network: bool,
    /// The filesystem root every read/write path must stay within.
    pub fs_root: PathBuf,
    /// Maximum size, in bytes, of a single file read/written (resource-exhaustion
    /// guard). `u64::MAX` effectively disables the cap.
    pub max_file_bytes: u64,
    /// When `Some`, a `Bash` command is only allowed if it begins with one of
    /// these prefixes (after trimming leading whitespace). `None` means "no
    /// prefix restriction" — any command is allowed *if* [`allow_bash`] is true.
    ///
    /// [`allow_bash`]: Capabilities::allow_bash
    pub allowed_bash_prefixes: Option<Vec<String>>,
}

impl Default for Capabilities {
    /// Deny-by-default sensible baseline: read+write within `fs_root`, `Bash`
    /// gated off, no network, a 16 MiB per-file cap. `fs_root` defaults to the
    /// current directory (`.`); callers should set it to the real sandbox root.
    fn default() -> Self {
        Capabilities {
            allow_read: true,
            allow_write: true,
            allow_bash: false,
            allow_network: false,
            fs_root: PathBuf::from("."),
            max_file_bytes: 16 * 1024 * 1024,
            allowed_bash_prefixes: None,
        }
    }
}

impl Capabilities {
    /// A permissive preset for trusted research environments: read, write, and
    /// `Bash` all enabled within `root`. Network stays denied (this layer never
    /// grants it) and the per-file cap is the default 16 MiB.
    pub fn permissive(root: impl Into<PathBuf>) -> Self {
        Capabilities {
            allow_read: true,
            allow_write: true,
            allow_bash: true,
            allow_network: false,
            fs_root: root.into(),
            ..Capabilities::default()
        }
    }

    /// The policy a SIA agent runs under **by default**, and the single
    /// enforcement point the native runners consult before every model-invoked
    /// tool call (issue #89).
    ///
    /// A SIA agent legitimately needs to read, write, and run shell commands in
    /// its workspace, so this profile grants `allow_read`/`allow_write`/
    /// `allow_bash`. It denies network (`allow_network = false`; this layer never
    /// grants it), confines all paths to `root` (`fs_root`), caps any single file
    /// at [`AGENT_DEFAULT_MAX_FILE_BYTES`] (10 MiB), and applies no bash
    /// allow-list (`allowed_bash_prefixes = None`, i.e. any command is permitted
    /// once `allow_bash` is satisfied).
    ///
    /// A **stricter profile** is a drop-in tightening of the returned value: set
    /// `allow_bash = false` to forbid shell entirely, set
    /// `allowed_bash_prefixes = Some(..)` to whitelist commands, or lower
    /// `max_file_bytes`. Because the runners enforce *this* profile at their
    /// tool-dispatch chokepoint, it is the one place the landlock/seccomp/WASI
    /// OS-level roadmap (see the module docs) builds on: tightening here
    /// tightens every native tool call.
    pub fn agent_default(root: impl Into<PathBuf>) -> Self {
        Capabilities {
            allow_read: true,
            allow_write: true,
            allow_bash: true,
            allow_network: false,
            fs_root: root.into(),
            max_file_bytes: AGENT_DEFAULT_MAX_FILE_BYTES,
            allowed_bash_prefixes: None,
        }
    }

    /// A read-only preset: reads within `root` are allowed; writes and `Bash` are
    /// denied. Useful for inspection-only tool surfaces.
    pub fn read_only(root: impl Into<PathBuf>) -> Self {
        Capabilities {
            allow_read: true,
            allow_write: false,
            allow_bash: false,
            allow_network: false,
            fs_root: root.into(),
            ..Capabilities::default()
        }
    }

    /// Reject any path that escapes `fs_root` via `..` components or by being
    /// absolute. Purely lexical (mirrors `resolve_in_sandbox` in the tool layer),
    /// so it is safe for not-yet-existing paths. Does **not** resolve symlinks —
    /// see `SECURITY.md` for that limitation and the landlock roadmap.
    pub fn check_within_root(&self, path: impl AsRef<Path>) -> Result<(), CapabilityError> {
        let path = path.as_ref();
        if path.is_absolute() {
            return Err(CapabilityError::AbsolutePath {
                path: path.display().to_string(),
            });
        }

        let mut depth: i32 = 0;
        for component in path.components() {
            match component {
                Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return Err(CapabilityError::Escape {
                            path: path.display().to_string(),
                        });
                    }
                }
                Component::Normal(_) => depth += 1,
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => {
                    return Err(CapabilityError::Escape {
                        path: path.display().to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Permit a read of `path`: requires `allow_read` and in-root containment.
    pub fn check_read(&self, path: impl AsRef<Path>) -> Result<(), CapabilityError> {
        if !self.allow_read {
            return Err(CapabilityError::ReadDenied {
                path: path.as_ref().display().to_string(),
            });
        }
        self.check_within_root(path)
    }

    /// Permit a write to `path`: requires `allow_write` and in-root containment.
    pub fn check_write(&self, path: impl AsRef<Path>) -> Result<(), CapabilityError> {
        if !self.allow_write {
            return Err(CapabilityError::WriteDenied {
                path: path.as_ref().display().to_string(),
            });
        }
        self.check_within_root(path)
    }

    /// Permit a `Bash` command: requires `allow_bash`, and — when
    /// `allowed_bash_prefixes` is set — that the command (after trimming leading
    /// whitespace) begins with one of the allowed prefixes.
    pub fn check_bash(&self, command: &str) -> Result<(), CapabilityError> {
        if !self.allow_bash {
            return Err(CapabilityError::BashDenied {
                command: command.to_string(),
            });
        }
        if let Some(prefixes) = &self.allowed_bash_prefixes {
            let trimmed = command.trim_start();
            let matched = prefixes.iter().any(|p| trimmed.starts_with(p.as_str()));
            if !matched {
                return Err(CapabilityError::BashPrefixNotAllowed {
                    command: command.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Enforce the per-file byte cap (resource-exhaustion guard).
    pub fn check_size(&self, nbytes: u64) -> Result<(), CapabilityError> {
        if nbytes > self.max_file_bytes {
            return Err(CapabilityError::SizeExceeded {
                nbytes,
                limit: self.max_file_bytes,
            });
        }
        Ok(())
    }
}

/// Why a capability check failed. `Display` messages always name the offending
/// path or command so the failure is actionable in logs and tool-result text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// A path was absolute; paths must be relative to `fs_root`.
    AbsolutePath { path: String },
    /// A path escaped `fs_root` (via `..` or a root component).
    Escape { path: String },
    /// Read capability is not granted.
    ReadDenied { path: String },
    /// Write capability is not granted.
    WriteDenied { path: String },
    /// `Bash` is disabled entirely.
    BashDenied { command: String },
    /// `Bash` is enabled but the command matched no allowed prefix.
    BashPrefixNotAllowed { command: String },
    /// A file exceeded the configured `max_file_bytes`.
    SizeExceeded { nbytes: u64, limit: u64 },
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityError::AbsolutePath { path } => write!(
                f,
                "path '{path}' is absolute; paths must be relative to the sandbox root"
            ),
            CapabilityError::Escape { path } => {
                write!(f, "path '{path}' escapes the sandbox root")
            }
            CapabilityError::ReadDenied { path } => {
                write!(f, "read capability denied for path '{path}'")
            }
            CapabilityError::WriteDenied { path } => {
                write!(f, "write capability denied for path '{path}'")
            }
            CapabilityError::BashDenied { command } => {
                write!(f, "bash capability denied for command '{command}'")
            }
            CapabilityError::BashPrefixNotAllowed { command } => write!(
                f,
                "bash command '{command}' is not in the allowed-prefix list"
            ),
            CapabilityError::SizeExceeded { nbytes, limit } => {
                write!(f, "file size {nbytes} bytes exceeds the {limit}-byte limit")
            }
        }
    }
}

impl Error for CapabilityError {}

// ## OS-level enforcement (stage 2): landlock — roadmap, intentionally not wired
//
// A thin Linux `landlock` integration would live here behind a non-default
// `landlock-sandbox` cargo feature, restricting the calling thread to
// `Capabilities::fs_root` with kernel-enforced rules (so confinement survives a
// bypass of the in-process allow-list). It is deliberately **not** implemented
// here because the `landlock` crate is not available in this build's offline
// dependency cache, and the issue's hard gates forbid adding a non-cache-available
// dependency. The intended shape (for when it can be vendored) is roughly:
//
// ```ignore
// #[cfg(feature = "landlock-sandbox")]
// pub mod landlock_support {
//     use super::Capabilities;
//     use landlock::{Access, AccessFs, Ruleset, RulesetAttr,
//                    RulesetCreatedAttr, RulesetStatus, ABI};
//
//     pub fn apply(caps: &Capabilities) -> Result<RulesetStatus, landlock::RulesetError> {
//         let abi = ABI::V1;
//         let access = if caps.allow_write { AccessFs::from_all(abi) }
//                      else { AccessFs::from_read(abi) };
//         Ok(Ruleset::default()
//             .handle_access(AccessFs::from_all(abi))?
//             .create()?
//             .add_rules(landlock::path_beneath_rules([&caps.fs_root], access))?
//             .restrict_self()?
//             .ruleset)
//     }
// }
// ```
//
// See `SECURITY.md` and the module-level roadmap for the full stage 2/3 plan
// (landlock + seccomp, then the WASI component model under wasmtime/wasi).

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/work")
    }

    #[test]
    fn check_within_root_accepts_in_root_paths() {
        let caps = Capabilities::default();
        assert!(caps.check_within_root("src/main.rs").is_ok());
        assert!(caps.check_within_root("a/b/c.txt").is_ok());
        // Internal `..` that nets back inside is fine.
        assert!(caps.check_within_root("sub/../top.txt").is_ok());
        assert!(caps.check_within_root("./x").is_ok());
    }

    #[test]
    fn check_within_root_rejects_parent_escape() {
        let caps = Capabilities::default();
        let err = caps.check_within_root("../secret.txt").unwrap_err();
        assert!(matches!(err, CapabilityError::Escape { .. }));
        assert!(err.to_string().contains("../secret.txt"));
        assert!(err.to_string().contains("escapes the sandbox root"));
        // Deep escape that dips below the root mid-path.
        assert!(caps.check_within_root("a/../../etc/passwd").is_err());
    }

    #[test]
    fn check_within_root_rejects_absolute_path() {
        let caps = Capabilities::default();
        let err = caps.check_within_root("/etc/passwd").unwrap_err();
        assert!(matches!(err, CapabilityError::AbsolutePath { .. }));
        assert!(err.to_string().contains("/etc/passwd"));
        assert!(err.to_string().contains("absolute"));
    }

    #[test]
    fn check_bash_denies_when_bash_disabled() {
        let caps = Capabilities::default(); // bash gated off by default
        let err = caps.check_bash("ls -la").unwrap_err();
        assert!(matches!(err, CapabilityError::BashDenied { .. }));
        assert!(err.to_string().contains("ls -la"));
    }

    #[test]
    fn check_bash_honors_allowed_prefixes() {
        let mut caps = Capabilities::permissive(root());
        caps.allowed_bash_prefixes = Some(vec!["cargo ".to_string(), "ls".to_string()]);

        // Matching prefixes (one with leading whitespace to exercise trim).
        assert!(caps.check_bash("cargo test").is_ok());
        assert!(caps.check_bash("  cargo build").is_ok());
        assert!(caps.check_bash("ls -la").is_ok());

        // Non-matching command is denied, with the command echoed back.
        let err = caps.check_bash("rm -rf /").unwrap_err();
        assert!(matches!(err, CapabilityError::BashPrefixNotAllowed { .. }));
        assert!(err.to_string().contains("rm -rf /"));
        assert!(err.to_string().contains("allowed-prefix list"));
    }

    #[test]
    fn check_bash_allows_any_command_when_no_prefix_list() {
        let caps = Capabilities::permissive(root()); // allowed_bash_prefixes = None
        assert!(caps.check_bash("anything goes here").is_ok());
    }

    #[test]
    fn check_read_and_write_honor_allow_flags() {
        let caps = Capabilities::read_only(root());
        // Read allowed in-root.
        assert!(caps.check_read("notes.txt").is_ok());
        // Write denied by the read-only preset, with the path in the message.
        let err = caps.check_write("notes.txt").unwrap_err();
        assert!(matches!(err, CapabilityError::WriteDenied { .. }));
        assert!(err.to_string().contains("notes.txt"));

        // A read-disabled capability denies reads.
        let mut no_read = Capabilities::permissive(root());
        no_read.allow_read = false;
        let err = no_read.check_read("notes.txt").unwrap_err();
        assert!(matches!(err, CapabilityError::ReadDenied { .. }));
        assert!(err.to_string().contains("notes.txt"));
    }

    #[test]
    fn check_read_write_still_enforce_containment() {
        let caps = Capabilities::permissive(root());
        assert!(caps.check_read("../escape").is_err());
        assert!(caps.check_write("/abs/path").is_err());
    }

    #[test]
    fn check_size_enforces_max_file_bytes() {
        let caps = Capabilities {
            max_file_bytes: 1024,
            ..Capabilities::default()
        };
        assert!(caps.check_size(0).is_ok());
        assert!(caps.check_size(1024).is_ok());
        let err = caps.check_size(1025).unwrap_err();
        assert!(matches!(err, CapabilityError::SizeExceeded { .. }));
        assert!(err.to_string().contains("1025"));
        assert!(err.to_string().contains("1024"));
    }

    #[test]
    fn default_is_deny_by_default_bash_and_network() {
        let caps = Capabilities::default();
        assert!(caps.allow_read);
        assert!(caps.allow_write);
        assert!(!caps.allow_bash);
        assert!(!caps.allow_network);
    }

    #[test]
    fn read_only_preset_behaves_as_expected() {
        let caps = Capabilities::read_only(root());
        assert!(caps.allow_read);
        assert!(!caps.allow_write);
        assert!(!caps.allow_bash);
        assert!(!caps.allow_network);
        assert_eq!(caps.fs_root, root());
        assert!(caps.check_read("a.txt").is_ok());
        assert!(caps.check_write("a.txt").is_err());
        assert!(caps.check_bash("ls").is_err());
    }

    #[test]
    fn agent_default_allows_workspace_ops_and_enforces_limits() {
        let caps = Capabilities::agent_default(root());
        // The agent profile grants read/write/bash within the workspace.
        assert!(caps.allow_read);
        assert!(caps.allow_write);
        assert!(caps.allow_bash);
        // Network is never granted by this layer.
        assert!(!caps.allow_network);
        assert_eq!(caps.fs_root, root());
        assert_eq!(caps.max_file_bytes, AGENT_DEFAULT_MAX_FILE_BYTES);
        assert!(caps.allowed_bash_prefixes.is_none());

        // Allowed: read/write/bash within root.
        assert!(caps.check_read("src/main.rs").is_ok());
        assert!(caps.check_write("out/result.txt").is_ok());
        assert!(caps.check_bash("cargo test").is_ok());

        // Denied: an oversize write (one byte over the cap).
        let err = caps
            .check_size(AGENT_DEFAULT_MAX_FILE_BYTES + 1)
            .unwrap_err();
        assert!(matches!(err, CapabilityError::SizeExceeded { .. }));
        // At the cap exactly is fine.
        assert!(caps.check_size(AGENT_DEFAULT_MAX_FILE_BYTES).is_ok());

        // Denied: a `..`-escaping path on both read and write.
        assert!(matches!(
            caps.check_read("../etc/passwd").unwrap_err(),
            CapabilityError::Escape { .. }
        ));
        assert!(matches!(
            caps.check_write("a/../../escape").unwrap_err(),
            CapabilityError::Escape { .. }
        ));
    }

    #[test]
    fn permissive_preset_behaves_as_expected() {
        let caps = Capabilities::permissive(root());
        assert!(caps.allow_read);
        assert!(caps.allow_write);
        assert!(caps.allow_bash);
        assert!(!caps.allow_network); // never granted by this layer
        assert_eq!(caps.fs_root, root());
        assert!(caps.check_read("a.txt").is_ok());
        assert!(caps.check_write("a.txt").is_ok());
        assert!(caps.check_bash("echo hi").is_ok());
    }
}
