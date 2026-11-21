READY TO SUBMIT: none yet (Phase 2 in progress)

# Progress

## Environment (Windows 11 laptop, 16 GB RAM)
- No WSL distribution installed, no Docker, no GitHub CLI. Per instructions none of these were installed.
  - The Docker image cannot be built or run locally; the Dockerfile is reviewed by hand against the scorer environment.
  - The repo is committed locally only. **It still needs a public GitHub repo (buildathon-shopping-sessions) and a push, including tags.**
- Rust: installed rustup via winget (stable, x86_64-pc-windows-msvc). Visual Studio Build Tools 2026 were already present, so the MSVC linker works.
- Python 3.14.3 with pyarrow and polars installed via pip --user.
- git 2.51 with global core.autocrlf=true; `.gitattributes` (`* -text`) overrides it for this repo.
- Datasets live in `../data/` (next to the repo, outside it). The path has no OneDrive in it.
- Generator built from `../generator` (stale Cargo.lock deleted): `../generator/target/release/generate.exe`.

## Decisions
- Column order is taken as fixed by rules.md (the generator always writes the same header); the first line of each file is skipped as the header whatever it contains (fixture L3 shows a later identical line is data).
- Input files: every regular file in the folder whose name ends `.csv` and does not start with `.`.
- `ingest_ts` is always valid F1 per rules.md; should one fail to parse, that copy sorts after every valid copy in dedup (and is then late, so dropped).
- Years in F1/F2 are exactly 4 ASCII digits; year 0000 is treated as bad (it is outside every window anyway).

## Phase status
### Phase 1: reference (done)
- `reference/reference.py`: plain Python + pyarrow, hand-written CSV parser.
- Fixtures: 36/36 pass (`python scripts/run_fixtures.py fixtures -- python reference/reference.py`).
- Dev set (seed 1, 1M events, 250k clicks): exact MATCH with dev-answer (117,038 user_daily rows, 27,400 campaign_daily rows). Reference runtime about 38 s.

### Phase 2: Rust pipeline (in progress)

### Phase 3: speed (not started)

## Timings per tag
(none yet)

## Open ideas
(none yet)
