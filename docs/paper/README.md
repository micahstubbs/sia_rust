# `docs/paper/` — academic preprint

This directory holds a preprint-style paper on the `sia_rust` project (issue #70).

## Build the PDF (academic LaTeX — this is the canonical build)

The paper is a genuine **arXiv single-column preprint**, built directly from LaTeX —
**not** from the `m2p` script/skill (which injects unrelated branding and a different
layout).

```sh
cd docs/paper
latexmk -pdf -interaction=nonstopmode sia_rust.tex   # runs pdflatex + bibtex
```

Files:

- [`sia_rust.tex`](sia_rust.tex) — the paper source (arXiv preprint via `arxiv.sty`).
- [`arxiv.sty`](arxiv.sty) — vendored arXiv preprint style (MIT,
  github.com/kourgeorge/arxiv-style).
- [`references.bib`](references.bib) — bibliography (cited with `natbib`/`unsrtnat`).
- `sia_rust.pdf` — the compiled paper.
- [`sia_rust_preprint.md`](sia_rust_preprint.md) — the original Markdown **content
  draft** and prose source-of-record; edit it and the `.tex` together as the
  implementation evolves.
- `archive/` — the obsolete, m2p-generated (branded) outputs, kept for history.

Before considering a build done, verify the `latexmk` log has **0 overfull hboxes, 0
LaTeX errors, 0 Unicode/citation warnings**, then visually check the page renders
(`pdftoppm -png -r 130 sia_rust.pdf pages/p`).

## Scope and accuracy policy

This is an academic draft about a real codebase. It describes **only** what is
implemented, cites **only** results that exist in the repo (byte-parity tests and
the microbenchmarks in `benchmarks/REPORT.md`), and invents no accuracy gains,
ablations, or benchmark numbers. Unimplemented or not-yet-evaluated work (full
meta-RL scheduling, GPU LoRA training, OS-level WASI/landlock enforcement, and live
end-to-end self-improvement studies) is explicitly marked future work. Keep new
claims sourced to code or in-repo artifacts.

The author/affiliation and the `hebbar2026sia` author list are placeholders to be
completed against the arXiv record before any external submission.
