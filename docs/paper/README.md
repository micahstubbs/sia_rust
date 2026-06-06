# `docs/paper/` — preprint draft

This directory holds an **iterative working draft** of a preprint-style paper on
the `sia_rust` project (issue #70).

- [`sia_rust_preprint.md`](sia_rust_preprint.md) — the draft. It is written as a
  **system / design / experience report**, not a results paper. Every contribution
  is tied to the module that implements it, and every quantitative claim is sourced
  to a repository artifact (the differential-parity harness and
  `benchmarks/REPORT.md`). Unimplemented or not-yet-evaluated work (full meta-RL
  scheduling, GPU LoRA training, OS-level WASI/landlock enforcement, and live
  end-to-end self-improvement studies) is explicitly marked future work.

## Scope and accuracy policy

This is an academic draft about a real codebase. It describes **only** what is
implemented, cites **only** results that exist in the repo (byte-parity tests and
the microbenchmarks in `benchmarks/REPORT.md`), and invents no accuracy gains,
ablations, or benchmark numbers. Update it as the implementation evolves; keep new
claims sourced to code or in-repo artifacts.

## Converting to LaTeX (optional, later)

The draft is plain Markdown with a BibTeX block in the References section. If/when
it is converted to LaTeX (e.g. for arXiv submission):

1. Split the BibTeX block into a `references.bib` file.
2. Convert the Markdown body to LaTeX, e.g. with Pandoc:

   ```sh
   pandoc sia_rust_preprint.md \
     --bibliography=references.bib \
     --citeproc \
     -o sia_rust_preprint.tex
   ```

3. Drop the resulting body into an arXiv-friendly template (e.g. `article` or a
   venue style), replace the inline ASCII architecture diagram with a proper
   figure, and complete the author list on the SIA citation against the arXiv
   record before submission.

No LaTeX toolchain is required to read or review the current draft.
