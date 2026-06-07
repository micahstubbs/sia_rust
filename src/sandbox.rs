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
//!    confinement that survives a logic bug in the tool layer. The **Landlock
//!    filesystem** half of this stage is implemented in [`landlock_support`],
//!    behind the non-default `landlock-sandbox` cargo feature, and degrades to a
//!    logged no-op off Linux / on kernels without Landlock. The seccomp network
//!    filter remains roadmap.
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

// ===========================================================================
// Security-event surfacing (issue #140)
// ===========================================================================
//
// The capability layer makes *decisions*; this section makes those decisions
// *observable*. A [`SecurityEvent`] records a single capability request and its
// outcome (allowed / denied), and a [`SecurityLog`] accumulates them so the
// orchestrator can fold them into the trajectory / telemetry the Feedback Agent
// reads. This is pure `std`, on the default build: observability must never
// depend on the optional OS-enforcement feature.

/// The kind of operation a [`SecurityEvent`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityAction {
    /// A filesystem read was requested.
    Read,
    /// A filesystem write was requested.
    Write,
    /// A `Bash` command was requested.
    Bash,
    /// A per-file size limit was checked.
    Size,
    /// An OS-level sandbox (e.g. Landlock) was applied to the process/thread.
    SandboxApply,
}

impl SecurityAction {
    /// Stable lowercase tag used in serialized events.
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityAction::Read => "read",
            SecurityAction::Write => "write",
            SecurityAction::Bash => "bash",
            SecurityAction::Size => "size",
            SecurityAction::SandboxApply => "sandbox_apply",
        }
    }
}

/// The outcome recorded on a [`SecurityEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityOutcome {
    /// The capability check passed.
    Allowed,
    /// The capability check failed (a violation / denial).
    Denied,
}

impl SecurityOutcome {
    /// Stable lowercase tag used in serialized events.
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityOutcome::Allowed => "allowed",
            SecurityOutcome::Denied => "denied",
        }
    }
}

/// A single structured security event: which capability was requested, against
/// what target, and whether it was allowed or denied (with the denial reason).
///
/// Events are intentionally string-typed so they serialize cleanly into the
/// existing JSON trajectory/telemetry surfaces without pulling `serde` into the
/// capability layer's signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityEvent {
    /// The operation class (read / write / bash / size / sandbox_apply).
    pub action: SecurityAction,
    /// Allowed or denied.
    pub outcome: SecurityOutcome,
    /// The path or command the request was about (empty for `Size`).
    pub target: String,
    /// On a denial, the human-readable reason (the `CapabilityError` message);
    /// empty when allowed.
    pub detail: String,
}

impl SecurityEvent {
    /// Build an `allowed` event for `action` against `target`.
    pub fn allowed(action: SecurityAction, target: impl Into<String>) -> Self {
        SecurityEvent {
            action,
            outcome: SecurityOutcome::Allowed,
            target: target.into(),
            detail: String::new(),
        }
    }

    /// Build a `denied` event for `action` against `target`, carrying `detail`
    /// (typically the `CapabilityError` `Display` message).
    pub fn denied(
        action: SecurityAction,
        target: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        SecurityEvent {
            action,
            outcome: SecurityOutcome::Denied,
            target: target.into(),
            detail: detail.into(),
        }
    }

    /// Render as a `serde_json::Value` object for trajectory/telemetry embedding.
    ///
    /// Shape: `{"action","outcome","target","detail"}` with stable lowercase
    /// tags. `detail` is omitted when empty to keep allowed events compact.
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("action".into(), self.action.as_str().into());
        obj.insert("outcome".into(), self.outcome.as_str().into());
        obj.insert("target".into(), self.target.clone().into());
        if !self.detail.is_empty() {
            obj.insert("detail".into(), self.detail.clone().into());
        }
        serde_json::Value::Object(obj)
    }
}

/// An append-only log of [`SecurityEvent`]s for one run/generation.
///
/// The orchestrator (or a native tool layer) records events here, then folds
/// them into the trajectory / telemetry the Feedback Agent reads, so that
/// capability denials — the signal that an agent attempted something outside
/// its allow-list — are visible to the self-improvement loop.
#[derive(Debug, Clone, Default)]
pub struct SecurityLog {
    events: Vec<SecurityEvent>,
}

impl SecurityLog {
    /// Create an empty log.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one event.
    pub fn record(&mut self, event: SecurityEvent) {
        self.events.push(event);
    }

    /// Borrow the recorded events.
    pub fn events(&self) -> &[SecurityEvent] {
        &self.events
    }

    /// The number of `Denied` events recorded (i.e. capability violations).
    pub fn violation_count(&self) -> usize {
        self.events
            .iter()
            .filter(|e| e.outcome == SecurityOutcome::Denied)
            .count()
    }

    /// Render every event as a JSON array for trajectory/telemetry embedding.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Array(self.events.iter().map(SecurityEvent::to_json).collect())
    }
}

impl Capabilities {
    /// Run [`check_read`](Self::check_read) and record the outcome to `log` as a
    /// [`SecurityEvent`].
    ///
    /// This is the *observable* variant: the decision is identical, but the
    /// request and its result are surfaced into `log` so the Feedback Agent can
    /// see allow-list hits and violations.
    pub fn check_read_logged(
        &self,
        path: impl AsRef<Path>,
        log: &mut SecurityLog,
    ) -> Result<(), CapabilityError> {
        let target = path.as_ref().display().to_string();
        let result = self.check_read(path.as_ref());
        self.record_outcome(SecurityAction::Read, target, log, result)
    }

    /// Logged variant of [`check_write`](Self::check_write).
    pub fn check_write_logged(
        &self,
        path: impl AsRef<Path>,
        log: &mut SecurityLog,
    ) -> Result<(), CapabilityError> {
        let target = path.as_ref().display().to_string();
        let result = self.check_write(path.as_ref());
        self.record_outcome(SecurityAction::Write, target, log, result)
    }

    /// Logged variant of [`check_bash`](Self::check_bash).
    pub fn check_bash_logged(
        &self,
        command: &str,
        log: &mut SecurityLog,
    ) -> Result<(), CapabilityError> {
        let result = self.check_bash(command);
        self.record_outcome(SecurityAction::Bash, command.to_string(), log, result)
    }

    /// Fold a `check_*` result into a [`SecurityEvent`] on `log`, then return it
    /// unchanged. Centralizes the allowed/denied bookkeeping for the `*_logged`
    /// methods so each call site stays a one-liner.
    fn record_outcome(
        &self,
        action: SecurityAction,
        target: String,
        log: &mut SecurityLog,
        result: Result<(), CapabilityError>,
    ) -> Result<(), CapabilityError> {
        match &result {
            Ok(()) => log.record(SecurityEvent::allowed(action, target)),
            Err(e) => log.record(SecurityEvent::denied(action, target, e.to_string())),
        }
        result
    }
}

// ===========================================================================
// OS-level enforcement (stage 2): Landlock filesystem confinement (issue #140)
// ===========================================================================
//
// `landlock_support` (behind the non-default `landlock-sandbox` feature) applies
// a kernel-enforced filesystem ruleset that confines the *calling thread and its
// future children* to `Capabilities::fs_root`, so confinement survives a logic
// bug or prompt-injection that bypasses the in-process allow-list above. It is
// Linux-only and **degrades to a logged no-op** on other platforms or on kernels
// that lack Landlock — it never breaks the build, the CI runner, or a run.

/// Outcome of attempting to apply an OS-level sandbox.
///
/// Returned by [`landlock_support::apply`] so callers can record what actually
/// happened (kernel enforced it, the kernel partially supports it, or the
/// platform/kernel can't enforce it and we degraded to a no-op) into a
/// [`SecurityLog`] / the trajectory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxStatus {
    /// The ruleset was fully enforced by the kernel.
    Enforced,
    /// The kernel supports Landlock but only enforces a subset of the requested
    /// access rights (an older Landlock ABI than we requested).
    PartiallyEnforced,
    /// No enforcement was applied (non-Linux target, or a kernel without
    /// Landlock). The in-process allow-list remains the only guard.
    NotSupported,
}

impl SandboxStatus {
    /// Stable lowercase tag for logs/telemetry.
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxStatus::Enforced => "enforced",
            SandboxStatus::PartiallyEnforced => "partially_enforced",
            SandboxStatus::NotSupported => "not_supported",
        }
    }

    /// Whether the kernel applied at least some enforcement.
    pub fn is_enforced(self) -> bool {
        matches!(
            self,
            SandboxStatus::Enforced | SandboxStatus::PartiallyEnforced
        )
    }
}

/// Linux Landlock filesystem confinement (feature `landlock-sandbox`).
///
/// On non-Linux targets or without the feature, [`apply`](landlock_support::apply)
/// is a no-op returning [`SandboxStatus::NotSupported`]; on Linux it restricts the
/// calling thread to read/write only beneath `Capabilities::fs_root`, honoring the
/// `allow_write` flag (read-only when writes are denied).
pub mod landlock_support {
    use super::{Capabilities, SandboxStatus};

    /// Apply a Landlock ruleset confining the calling thread to `caps.fs_root`.
    ///
    /// Returns the [`SandboxStatus`] the kernel reported. This call is best-effort
    /// confinement: it tightens the OS boundary but the in-process capability
    /// allow-list remains the policy source of truth.
    ///
    /// # Degradation
    ///
    /// Without `--features landlock-sandbox`, or on any non-Linux target, this is
    /// a no-op that logs a warning and returns [`SandboxStatus::NotSupported`] —
    /// it never errors and never breaks a run.
    #[cfg(all(feature = "landlock-sandbox", target_os = "linux"))]
    pub fn apply(caps: &Capabilities) -> SandboxStatus {
        use landlock::{
            Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
            RulesetStatus, ABI,
        };

        // Use the broadest ABI we were compiled against; `landlock` clamps the
        // requested access to what the running kernel actually supports and
        // reports that back via `RulesetStatus`.
        let abi = ABI::V1;
        let read_only = AccessFs::from_read(abi);
        let read_write = AccessFs::from_all(abi);
        let granted = if caps.allow_write {
            read_write
        } else {
            read_only
        };

        // Opening the root as an fd can fail if it does not exist yet; that is a
        // caller error, but we degrade rather than abort the run.
        let path_fd = match PathFd::new(&caps.fs_root) {
            Ok(fd) => fd,
            Err(e) => {
                log::warn!(
                    "landlock: cannot open fs_root {}: {e}; running without OS sandbox",
                    caps.fs_root.display()
                );
                return SandboxStatus::NotSupported;
            }
        };

        let result = Ruleset::default()
            .handle_access(read_write)
            .and_then(|r| r.create())
            .and_then(|r| r.add_rule(PathBeneath::new(path_fd, granted)))
            .and_then(|r| r.restrict_self());

        match result {
            Ok(status) => match status.ruleset {
                RulesetStatus::FullyEnforced => SandboxStatus::Enforced,
                RulesetStatus::PartiallyEnforced => SandboxStatus::PartiallyEnforced,
                RulesetStatus::NotEnforced => {
                    log::warn!(
                        "landlock: kernel reported NotEnforced for {}; \
                         running without OS sandbox",
                        caps.fs_root.display()
                    );
                    SandboxStatus::NotSupported
                }
            },
            Err(e) => {
                log::warn!(
                    "landlock: failed to apply ruleset for {}: {e}; \
                     running without OS sandbox",
                    caps.fs_root.display()
                );
                SandboxStatus::NotSupported
            }
        }
    }

    /// No-op fallback: the feature is on but the target is not Linux.
    #[cfg(all(feature = "landlock-sandbox", not(target_os = "linux")))]
    pub fn apply(caps: &Capabilities) -> SandboxStatus {
        log::warn!(
            "landlock: not supported on this platform (only Linux); \
             {} is guarded only by the in-process allow-list",
            caps.fs_root.display()
        );
        SandboxStatus::NotSupported
    }

    /// No-op fallback: the `landlock-sandbox` feature is disabled.
    #[cfg(not(feature = "landlock-sandbox"))]
    pub fn apply(caps: &Capabilities) -> SandboxStatus {
        log::warn!(
            "landlock: built without the `landlock-sandbox` feature; \
             {} is guarded only by the in-process allow-list",
            caps.fs_root.display()
        );
        SandboxStatus::NotSupported
    }
}

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

    // --- security-event surfacing (issue #140) ---------------------------

    #[test]
    fn logged_read_records_allowed_event() {
        let caps = Capabilities::permissive(root());
        let mut log = SecurityLog::new();
        assert!(caps.check_read_logged("src/a.rs", &mut log).is_ok());

        assert_eq!(log.events().len(), 1);
        let ev = &log.events()[0];
        assert_eq!(ev.action, SecurityAction::Read);
        assert_eq!(ev.outcome, SecurityOutcome::Allowed);
        assert_eq!(ev.target, "src/a.rs");
        assert!(ev.detail.is_empty());
        assert_eq!(log.violation_count(), 0);
    }

    #[test]
    fn logged_write_records_denial_with_reason() {
        let caps = Capabilities::read_only(root());
        let mut log = SecurityLog::new();
        let err = caps.check_write_logged("out.txt", &mut log).unwrap_err();
        assert!(matches!(err, CapabilityError::WriteDenied { .. }));

        assert_eq!(log.events().len(), 1);
        let ev = &log.events()[0];
        assert_eq!(ev.action, SecurityAction::Write);
        assert_eq!(ev.outcome, SecurityOutcome::Denied);
        assert_eq!(ev.target, "out.txt");
        // The denial reason is the CapabilityError message.
        assert!(ev.detail.contains("write capability denied"));
        assert_eq!(log.violation_count(), 1);
    }

    #[test]
    fn logged_bash_escape_is_recorded_as_violation() {
        let mut caps = Capabilities::permissive(root());
        caps.allowed_bash_prefixes = Some(vec!["cargo ".to_string()]);
        let mut log = SecurityLog::new();

        assert!(caps.check_bash_logged("cargo test", &mut log).is_ok());
        assert!(caps
            .check_bash_logged("curl evil.example", &mut log)
            .is_err());

        assert_eq!(log.events().len(), 2);
        assert_eq!(log.violation_count(), 1);
        assert_eq!(log.events()[1].outcome, SecurityOutcome::Denied);
        assert_eq!(log.events()[1].target, "curl evil.example");
    }

    #[test]
    fn security_event_json_shape_is_stable() {
        let allowed = SecurityEvent::allowed(SecurityAction::Read, "a.txt");
        assert_eq!(
            allowed.to_json(),
            serde_json::json!({"action": "read", "outcome": "allowed", "target": "a.txt"})
        );

        let denied = SecurityEvent::denied(SecurityAction::Bash, "rm -rf /", "bash denied");
        assert_eq!(
            denied.to_json(),
            serde_json::json!({
                "action": "bash",
                "outcome": "denied",
                "target": "rm -rf /",
                "detail": "bash denied"
            })
        );
    }

    #[test]
    fn security_log_to_json_is_an_array_of_events() {
        let caps = Capabilities::read_only(root());
        let mut log = SecurityLog::new();
        let _ = caps.check_read_logged("ok.txt", &mut log);
        let _ = caps.check_write_logged("nope.txt", &mut log);

        let json = log.to_json();
        let arr = json.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["outcome"], "allowed");
        assert_eq!(arr[1]["outcome"], "denied");
    }

    #[test]
    fn action_and_outcome_tags_are_lowercase_stable() {
        assert_eq!(SecurityAction::Read.as_str(), "read");
        assert_eq!(SecurityAction::Write.as_str(), "write");
        assert_eq!(SecurityAction::Bash.as_str(), "bash");
        assert_eq!(SecurityAction::Size.as_str(), "size");
        assert_eq!(SecurityAction::SandboxApply.as_str(), "sandbox_apply");
        assert_eq!(SecurityOutcome::Allowed.as_str(), "allowed");
        assert_eq!(SecurityOutcome::Denied.as_str(), "denied");
    }

    // --- Landlock OS sandbox smoke test (issue #140) ----------------------

    #[test]
    fn sandbox_status_tags_and_is_enforced() {
        assert_eq!(SandboxStatus::Enforced.as_str(), "enforced");
        assert_eq!(
            SandboxStatus::PartiallyEnforced.as_str(),
            "partially_enforced"
        );
        assert_eq!(SandboxStatus::NotSupported.as_str(), "not_supported");
        assert!(SandboxStatus::Enforced.is_enforced());
        assert!(SandboxStatus::PartiallyEnforced.is_enforced());
        assert!(!SandboxStatus::NotSupported.is_enforced());
    }

    /// Smoke test for the Landlock apply path. It must *never* fail the suite:
    /// without the `landlock-sandbox` feature (the default + `llm` CI builds) it
    /// returns `NotSupported`; with the feature on Linux it returns whatever the
    /// running kernel supports (Enforced / PartiallyEnforced / NotSupported on an
    /// old kernel). We assert only that the call returns a valid status and never
    /// panics — confinement is gracefully skipped where unsupported.
    ///
    /// NOTE: this is the *last* `apply` we can meaningfully assert on in-process
    /// when enforcement succeeds, because `restrict_self()` is irreversible for
    /// the calling thread. Cargo runs each `#[test]` on its own thread, so this
    /// does not leak the restriction into sibling tests, but we still keep the
    /// applied root permissive (a temp dir) so nothing else this thread does
    /// afterwards is constrained in a surprising way.
    #[test]
    fn landlock_apply_returns_a_status_and_never_panics() {
        let dir = tempfile::tempdir().unwrap();
        let caps = Capabilities::read_only(dir.path().to_path_buf());
        let status = landlock_support::apply(&caps);
        // Any of the three variants is acceptable; the point is no panic / no
        // build break and that the status string is one of the known tags.
        assert!(matches!(
            status,
            SandboxStatus::Enforced
                | SandboxStatus::PartiallyEnforced
                | SandboxStatus::NotSupported
        ));
        assert!(["enforced", "partially_enforced", "not_supported"].contains(&status.as_str()));
    }
}
