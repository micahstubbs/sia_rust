# sia_rust Academic Paper

This directory contains the LaTeX source for the academic preprint describing the novel contributions of the sia_rust project.

## Goal
Produce a submission-ready preprint for arXiv (primary) that can also be adapted for relevant workshops or conferences.

## Template
We use the excellent `kourgeorge/arxiv-style` (https://github.com/kourgeorge/arxiv-style), a clean, single-column NeurIPS-inspired style specifically designed for preprints. It avoids the common mistake of readers thinking the paper is already published in a conference.

## Files
- `arxiv.sty` — The style file (do not modify core geometry/fonts unless necessary).
- `main.tex` — The main paper source (start here).
- This README.

## Compilation (local)

```bash
pdflatex main
pdflatex main
pdflatex main   # for references
```

Or use the provided `Makefile` (to be added) or Overleaf.

## arXiv Submission Rules (Critical)

See the detailed research comment on GitHub issue #70 for the full checklist. Key points:

- Generate `.bbl` locally and inline it (arXiv does **not** run BibTeX for you).
- Use only Type 1 or embedded OpenType fonts (avoid Matplotlib Type 3 fonts).
- Flatten includes; keep figures in root or relative paths.
- Remove all comments, auxiliary files, and hidden files before upload.
- Recommended categories: `cs.AI`, `cs.LG`, `cs.SE`.

## Mapping to Implementation Issues

The paper is structured to highlight these novel contributions (tracked in the linked issues):

- Native rig-core LLM client + trajectory middleware (#62, #51, #46)
- Adaptive harness/weight scheduler (#65)
- SIA Studio live dashboard (#63)
- Nebius GPU + telemetry integration (#64, #69)
- Security / threat model (#67)
- Domain templates + robust verifiers (#66)
- Hackathon demo narrative (#68, #71)

## Next Steps for Contributors / Worker

1. Claim this work via issue #70.
2. Fill sections in `main.tex` in parallel with code implementation.
3. Keep the abstract and introduction updated as results come in.
4. When ready, open a PR from this branch (`docs/add-paper-skeleton`) or merge into main.

## License
MIT (same as sia_rust).