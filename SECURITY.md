# SIA Threat Model & Security

SIA is a **self-improving / self-modifying agent framework**: it spawns AI agents
that read, write, and execute code on the host. That makes the execution sandbox a
first-class security concern rather than an afterthought. This document is the
formal threat model for the Rust port — assets, trust boundaries, what a generated
or target agent can do, the escalation paths we worry about, the mitigations in
place today, and the roadmap to OS-level enforcement.

The framework-track thesis is that the Rust port is *not just faster — it is a
sounder substrate for safe self-modification*. The honest current state: we have a
clear threat model, a lexical filesystem sandbox in the native tool layer, an
optional Docker jail for the Python target agent, and a new pure-`std` **capability
allow-list** (`src/sandbox.rs`) that gives tool executors a single, auditable
enforcement point. Kernel-/VM-level enforcement (landlock, seccomp, WASI) is the
documented roadmap, not yet shipped.

---

## 1. Assets

What an attacker (or a buggy/injected agent) could try to reach or abuse:

- **Host filesystem** outside the intended workspace — source trees, SSH keys,
  `~/.aws`, `/etc`, other users' data.
- **Secrets in the environment** — `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, cloud
  credentials inherited by spawned subprocesses.
- **Network egress** — used for data exfiltration (uploading stolen secrets/data)
  or for pulling in further attacker-controlled payloads.
- **Host compute** — CPU/memory/disk; a target agent that exhausts these is a
  denial-of-service against the operator's machine.
- **Run integrity** — the trajectory logs, `results.json`, and context that drive
  the next generation; corrupting these poisons the self-improvement loop.

## 2. Trust boundaries

There are three distinct execution contexts with very different trust postures:

1. **Orchestrator (trusted).** `src/orchestrator.rs`, `src/run.rs`, `src/cli.rs`,
   `src/config.rs`. This is the operator's own code. It loads logs, builds prompts,
   and spawns the other two contexts. We trust it; it is the policy decision point.

2. **Native runner tools (semi-trusted; LLM-driven).** With `--features llm`, the
   native Claude/feedback runner exposes `Bash`/`Read`/`Write`/`Edit`/`Glob`
   executors in `src/llm/tools.rs`. The *code* is ours, but the *arguments* are
   chosen by an LLM that can be steered by task data or prompt injection. This is
   the primary self-modifying-code attack surface for native runs: the model can
   ask to write arbitrary files and run arbitrary shell commands within the
   sandbox.

3. **Python target subprocess (untrusted).** The generated `target_agent.py` is
   *machine-written code* executed each generation
   (`run_target_agent_with` → `build_target_cmd` / `build_sandbox_cmd` in
   `src/orchestrator.rs`). It should be treated as untrusted: it is exactly the
   "self-modifying" output we are iterating on, and a bad generation (or a
   prompt-injected one) can contain anything.

The boundary that matters most: **untrusted generated code (contexts 2 & 3) must
not be able to reach the assets in §1 beyond an explicit, declared allow-list.**

## 3. What a generated / target agent can do

- **Native runner (`src/llm/tools.rs`).**
  - `Read`/`Write`/`Edit`/`Glob` take a path argument resolved under `working_dir`
    by `resolve_in_sandbox`, which rejects absolute paths and `..` escapes
    *lexically*.
  - `Bash` runs **arbitrary** shell commands via `sh -c` in `working_dir` with a
    wall-clock timeout. There is no command allow-list and no network restriction
    at this layer — `Bash` is the widest hole.

- **Python target subprocess (`src/orchestrator.rs`).**
  - In the default `--sandbox none` mode it runs as a plain subprocess with the
    operator's full ambient authority: full host filesystem (within OS perms),
    full network, and the inherited environment (including API keys).
  - In `--sandbox docker` mode it is confined to a container (see §5).

## 4. Escalation paths

- **Filesystem escape.**
  - *Native tools:* The lexical sandbox in `resolve_in_sandbox` blocks `..` and
    absolute paths, **but does not resolve symlinks** — a symlink already present
    in `working_dir` that points outside could be followed by `Read`/`Write`. The
    `Bash` tool sidesteps the path sandbox entirely (`cat /etc/passwd`, `cd ..`).
  - *Target subprocess:* In `--sandbox none` there is no filesystem confinement at
    all.
- **Network exfiltration.** Native `Bash` (`curl`, `nc`) and the `--sandbox none`
  target subprocess have unrestricted egress; either can upload secrets or data.
- **Resource exhaustion.** A target agent can fork-bomb, allocate unbounded memory,
  or fill the disk. Native `Bash` has a wall-clock timeout but no memory/CPU cap.
- **Prompt-injection-driven tool abuse.** Task data, dataset contents, or prior
  trajectory text can carry instructions that steer the LLM into invoking tools
  maliciously ("read the SSH key and `curl` it to evil.example"). Because tool
  *arguments* come from the model, injection upgrades to real host actions wherever
  a tool is unconstrained — most acutely via `Bash`.

## 5. Current mitigations

- **Lexical path sandbox** — `resolve_in_sandbox` in `src/llm/tools.rs` rejects
  absolute paths and `..`/root escapes for every native file tool, and is applied
  *before* any filesystem touch (safe for not-yet-existing write targets). Covered
  by tests (`sandbox_rejects_parent_escape`, `sandbox_rejects_absolute_path`,
  `sandbox_allows_internal_dotdot_that_stays_inside`).
- **`Bash` wall-clock timeout** — the native `Bash` executor kills runaway children
  after `timeout_secs` (test: `bash_times_out_and_kills_child`), bounding one
  resource-exhaustion vector.
- **Optional Docker jail for the target subprocess** — `--sandbox docker`
  (`build_sandbox_cmd`) runs the generated agent with:
  - `--network none` (no egress),
  - dataset mounted **read-only** (`/data:ro`), workspace **read-write**
    (`/work:rw`),
  - `--memory` and `--cpus` limits from `Config`,
  - no host environment / filesystem access.
  This is the recommended mode for untrusted tasks.
- **Capability allow-list (`src/sandbox.rs`, this port).** A pure-`std`,
  dependency-free policy layer the native tool executors can consult *before*
  acting:
  - `Capabilities { allow_read, allow_write, allow_bash, allow_network, fs_root,
    max_file_bytes, allowed_bash_prefixes }` with a **deny-by-default** `Default`
    (read/write within `fs_root`, `Bash` gated, no network, 16 MiB/file cap) plus
    `permissive(root)` and `read_only(root)` presets.
  - `check_read` / `check_write` / `check_bash` / `check_within_root` /
    `check_size` each return a `CapabilityError` whose message names the offending
    path or command. `check_bash` can additionally restrict commands to an
    allow-listed set of prefixes (e.g. only `cargo `, `ls`), directly mitigating
    the prompt-injection-via-`Bash` path.
  This is the single, testable enforcement point on which the OS-level roadmap
  builds. It is **advisory / in-process**: it raises the bar against injection and
  accidental escape but does not constrain a process that ignores it.

### Honest limitations (today)

- Native `Bash` still runs arbitrary commands and is **not yet wired** to the
  capability layer's `check_bash` — `src/sandbox.rs` is the enforcement *primitive*;
  wiring it into the live tool loop is intentionally deferred so existing behavior
  and tests are unchanged.
- The lexical sandbox **does not resolve symlinks** (TOCTOU / symlink traversal
  remain possible at the native file-tool layer).
- The capability layer is in-process and advisory; it is **not** kernel-enforced.
- `--sandbox none` is the default and offers no confinement for the target agent.
- The Claude SDK runner path uses `permission_mode="bypassPermissions"` (see §7),
  trading interactive approval for automation.

## 6. Roadmap — toward kernel/VM-enforced isolation

The capability allow-list is stage 1 of three; each later stage *enforces* the same
policy at a lower level so a logic bug or injection in the tool layer cannot simply
ignore it:

1. **Capability allow-list — shipped.** `src/sandbox.rs`. Pure `std`, advisory,
   in-process; the policy source of truth.
2. **OS sandboxing for native execution — planned.** On Linux, apply a
   [`landlock`](https://crates.io/crates/landlock) filesystem ruleset scoped to
   `fs_root` (unprivileged, per-thread, kernel-enforced) and a `seccomp` syscall
   filter (e.g. [`seccompiler`](https://crates.io/crates/seccompiler)) to block raw
   network syscalls when `allow_network` is false. Intended to live behind a
   non-default `landlock-sandbox` cargo feature so default/`llm` builds gain no
   dependency. A code sketch is kept in `src/sandbox.rs`; it is not wired in this
   build because the `landlock` crate is not available in the offline dependency
   cache (adding it would violate the no-new-mandatory-dep / cache-availability
   constraint).
3. **WASI component model — planned.** Run untrusted generated agents as WebAssembly
   components under [`wasmtime`](https://crates.io/crates/wasmtime) with
   [`wasi`](https://crates.io/crates/wasi) preview2 capabilities: only explicit
   preopened directories, no ambient network or process-spawn authority. This is
   the strongest target — capability-based isolation by construction — and is the
   natural endpoint for "fundamentally safer self-modifying agents."

---

## 7. Operational notes

### Execution modes (target subprocess)

| Mode | Flag | Isolation | Risk |
|------|------|-----------|------|
| Direct | `--sandbox none` (default) | None | High — full host authority |
| Docker | `--sandbox docker` | Container + `--network none` + ro dataset + cpu/mem caps | Low |

Direct mode is appropriate only for **trusted research environments** (controlled
task data, trusted model, an already-isolated host such as a disposable VM). Use
`--sandbox docker` for anything untrusted. Docker must be installed and runnable by
the user.

### `bypassPermissions` in the Claude SDK runner

The Claude Code SDK agent runner uses `permission_mode="bypassPermissions"` so
automated runs don't pause for human approval on every file operation. This is only
safe in a controlled workspace, with a trusted model, or behind the Docker sandbox.

### Reference agent templates

Files in `sia/tasks/_shared/` and `sia/tasks/*/reference/` are **template code**
the meta-agent reads as examples (they use `subprocess.run(shell=True)`); they are
not executed by the framework directly but rewritten into new target agents.

## 8. Reporting

Report security vulnerabilities to security@hexo.ai.
