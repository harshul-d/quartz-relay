READY TO SUBMIT: v8 fa0d6d2de35482cee9c7ab7f9b56262d0153cbc4 (v7's code with neutral names; same 20M timing as v7)

# Progress

## Environment (Windows 11 laptop: i5-1135G7, 4 cores / 8 threads, AVX-512, 16 GB RAM)
- No WSL distribution installed, no Docker, no GitHub CLI. Per instructions none of these were installed.
  - The Docker image cannot be built or run locally; the Dockerfile is reviewed by hand against the scorer environment (below).
  - The repo is committed locally only. **It still needs a public GitHub repo (quartz-relay) and a push, including tags** (`git push -u origin main --tags`).
- Rust: rustup via winget (stable 1.100, x86_64-pc-windows-msvc). Visual Studio Build Tools 2026 were already present, so the MSVC linker works. The `x86_64-unknown-linux-gnu` std is installed: with mimalloc temporarily removed, `cargo check --target x86_64-unknown-linux-gnu` with the Dockerfile's RUSTFLAGS type-checks all of our Linux code paths (mimalloc's C part needs the image's gcc).
- Python 3.14.3 with pyarrow, polars, duckdb and psutil (pip --user).
- git 2.51 with global core.autocrlf=true; `.gitattributes` (`* -text`) overrides it for this repo.
- Datasets live in `../data/` (next to the repo, outside it):
  - `dev/` seed 1, 1M events (the public dev set); `diff/` the differential datasets and cached reference outputs;
  - `s101-20m/` seed 101, 20M events, 5M clicks; `s102-60m/` seed 102, 60M events, 15M clicks;
  - `bins/` saved binaries of earlier versions for A/B timing.
- Generator built from `../generator` (stale Cargo.lock deleted): `../generator/target/release/generate.exe`.

## Scorer-time work (after v8)
Starting point: v8 scored 1.39 s on the scorer's hidden 20M practice data (leader 0.30 s, 10th ~0.59 s, 50th ~0.85 s). An independent pipeline also stalled at ~1.40 s, so the hypothesis to test is a shared floor (input reading, page faults, start-up/teardown, output) rather than parsing or session logic. Laptop timings are used for correctness only from here on; every speed decision comes from GitHub Actions.

### Benchmark setup (branch `perf`, workflow `.github/workflows/bench.yml`)
- Runs on pushes to `perf` and on manual dispatch, on `ubuntu-latest`. Results: `results/<sha>.md` on branch `bench` (read with `git fetch origin bench; git show origin/bench:results/<sha>.md`), also in the run's step summary and as notice annotations.
- Steps: machine info and CPU flags; datasets (dev seed 1, 1M/250k; bench seed 101, 20M events / 5M clicks) generated with `tools/gen` (a copy of the generator, neutral package name `datagen`, its test module removed because it used a crate that is not shipped; output verified byte-identical to the original for seed 7) and cached with actions/cache; the scorer's image (`docker build`); the 36 fixtures and the dev answer (`ci/dev-answer`, a copy of the public dev answer) through `scripts/docker_run.sh`; then `ci/bench.py`.
- If the runner lacks x86-64-v3 instructions the CI image is built for x86-64-v2 and the report says so; the scorer's Dockerfile is never changed for CI.
- `ci/bench.py`: builds one image per line of `ci/variants.txt` (`name git-ref [ENV=VALUE ...]`, extra env for flag-gated experiments), one untimed warm-up run each, then 5 repetitions; in each repetition every variant (rotated order) runs cold (sync + drop_caches) and warm with the flags of `scripts/docker_run.sh` (`--cpus` = runner CPUs), then `/bin/true` in the same image cold and warm (container overhead), then natively under `/usr/bin/time -v`; plus a raw `cat` read floor. Wall = docker run start to exit. Reports median (min-max), per-phase medians from the binary's stderr, and checks each variant's 20M output against the first variant.
- Phase log (always on, cost: one clock read per 2 MB chunk and per partition, an rdtsc pair per person): cumulative wall marks `lookups`, `links`, `events parsed`, `events deduplicated`, `clicks parsed`, `sessionized`, `written`, `end of main`, plus `cpu NAME SECONDS` counters summed over threads: `read` (positional reads), `group-events`, `group-clicks`, `sessions`, `attribution`. Time outside main (process start, exit/teardown, container) = wall - `end of main`.

### Baseline breakdown of v8 (Actions; runner hardware changes between runs, so only same-job numbers compare)
Run 464472a (AMD EPYC 7763, 4 vCPU = 2 cores x 2 threads, THP `madvise`), 20M seed 101 (1.89 GB), container, medians of 5:

| Phase | warm s | cold s |
| --- | --- | --- |
| lookups | 0.012 | 0.013 |
| ID linking | 0.112 | 0.110 |
| events read+parse | 1.084 | 3.850 |
| dedup | 0.074 | 0.076 |
| clicks read+parse | 0.189 | 0.522 |
| sessionize (group events 0.73 cpu-s, group clicks 0.31, sessions 0.17, attribution 0.46) | 0.419 | 0.417 |
| output writing | 0.043 | 0.041 |
| outside main (container start/stop, process start/exit) | 0.199 | 0.404 |
| **total wall** | **2.129** | **5.436** |

Same data: `docker run ... /bin/true` 0.166 s warm, 0.380 cold; natively the time outside main is 0.016-0.036 s; `cat` of all input 4.46 s cold (424 MB/s, no faster with 16 readers), 0.046 s warm. Native `/usr/bin/time -v`: max RSS 2.05 GB, 2,300 minor faults, user 6.9 s, system 0.5 s. First run (3e69c52, Intel Xeon 6975P, THP `always`): v8 1.72 s warm, 3.98 s cold.

### Hypotheses tested
| # | Hypothesis | Result | Decision |
| --- | --- | --- | --- |
| H1 | A shared floor outside parsing (reads, faults, start/exit, output) | Warm: outside-main 0.2 s of 2.1 s, of which 0.17 s is Docker itself (`/bin/true`); output 0.04 s. Cold: total = I/O floor + ~0.5 s. Parsing (events 51%, sessionize 20%) dominates warm. | Partly rejected: the floor exists only for cold reads. Since a 0.30 s leader cannot read 1.9 GB cold, the scorer is most likely warm; compute is the target. |
| H2 | Page faults from fresh memory | 2,300 minor faults for 2 GB RSS (mimalloc's large aligned segments get THP even under `madvise`); THP `madvise` vs `always`, `MIMALLOC_ALLOW_LARGE_OS_PAGES=1`: all within noise | Rejected |
| H3 | Process teardown/start | 0.02-0.04 s natively | Rejected (nothing to win) |
| H4 | Cold reads: `POSIX_FADV_WILLNEED` on every input file from a background thread at start | cold 5.17 s (5.14-5.19) vs 5.44 (5.41-5.51); warm 2.10 vs 2.13 (noise). Repeat (e22b0b9, EPYC 9V74): cold 5.55 vs 5.60, warm 2.08 vs 2.15 (spreads overlap) | Kept, default on (`PIPELINE_WILLNEED=0` disables): never worse |
| H6 | More threads than CPUs (NCPU=8 on 4 vCPU) | warm 2.12 vs 2.08, cold equal | Rejected: threads = CPUs |
| H5 | Container exit | e22b0b9: outside main 0.50 s in the container vs 0.13 s for `/bin/true`, 0.017 s natively. a02075b (epoch stamps at main start/end vs the host clock around `docker run`): start (run -> main) 0.10 s = `/bin/true`; **exit (end of main -> run returns) 0.41 s** warm and cold vs 0.007 s natively; with THP disabled for the process (`MIMALLOC_ALLOW_THP=0`) exit 0.50 s and native exit 0.09 s, and the whole run slower (2.58 vs 2.36 warm) | Real on some runners, ~18% of warm wall there: investigated below |
| H5a | Exit cost = memory teardown or exit-time handlers (5cbba27, EPYC 9V74) | Probe (touch 2 GB, exit): container exit 0.043 s. `PIPELINE_EXIT=_exit` (skip atexit, incl. mimalloc's exit collect): exit 0.42 s. `PIPELINE_EXIT=release` (purge to OS before exit): 0.39 s | Rejected: not memory, not atexit |
| H5b | Exit cost = the container's dirty output (~93 MB) flushed when the overlay root is unmounted (overlayfs syncs the upper filesystem) | 97edf3b landed on EPYC 7763, where head's exit is only 0.07 s: 93 MB write probe exit 0.06 s, fsync of the real output inside main +0.02 s | Inconclusive (fast-exit runner). The slow exit shows on EPYC 9V74 and Xeon 6973P runners only; the workflow now runs 3 identical shards per push to cover several machine types |

## Public clean-up (v8)
- The public repo will be named quartz-relay. Every tracked file name and content was cleared of the event, sponsor and track names listed in the clean-up request (checked case-insensitively, allowing spaces, hyphens and underscores between word parts, with `git grep -iE` over all tracked files: no hits). They are deliberately not spelled out here.
- Neutral names: package and binary `pipeline`, Docker image tag `pipeline:latest` (README, `scripts/docker_run.sh`), title "Click-stream sessionization pipeline". The Parquet writer's `created_by` metadata string is now `pipeline` (file metadata only; table contents are unchanged).
- `Cargo.lock` changed only by the package entry moving to its alphabetical place; no dependency versions changed.
- No commit message contained any of the words, so history was not rewritten and tags v1-v7 still point at their original commits. Note: the file contents of the older commits (v1-v7) still contain the words; they become public if the history or those tags are pushed.
- After the rename: unit tests 6/6, fixtures 36/36, dev answer MATCH, full differential suite (15 datasets) MATCH, pyarrow/DuckDB Parquet check OK.
- `../versions/v1` to `../versions/v8` (outside the repo) hold `git archive` snapshots of the tracked files at each tag.
- No git remote added; nothing pushed.

## Local machine notes
- 2026-11-21: another user's Python batch job (8-18 processes) saturated all CPUs for hours; later the laptop ran on battery (Balanced plan, throttled) with the search indexer busy. All speed decisions were made from interleaved A/B runs under the same conditions; absolute times vary 2x between sessions.

## How to check a change (all must pass before a tag)
- `powershell -File scripts/verify.ps1`: build, unit tests, 36 fixtures, dev answer, differential suite (15 datasets).
- `python scripts/check_parquet.py OUT`: pyarrow and DuckDB read the output identically to polars (schema, logical types).
- For big changes: compare with v1 on 20M/60M (`../data/bins/v1.exe`, an independent implementation) using `scripts/compare.py`.

## Decisions
- Column order is taken as fixed by rules.md (the generator always writes the same header); the first line of each file is skipped as the header whatever it contains (fixture L3 shows a later identical line is data).
- Input files: every regular file in the folder whose name ends `.csv` and does not start with `.`.
- `ingest_ts` is always valid F1 per rules.md; should one fail to parse, that copy sorts after every valid copy in dedup (and is then late, so dropped).
- Years in F1/F2 are exactly 4 ASCII digits; year 0000 is treated as bad (it is outside every window anyway). F3 values with more than 18 significant digits are outside every window.
- products.csv / campaigns.csv rows with an empty field are skipped; a repeated ID keeps its last row (both implementations).
- Clicks are validated before dedup: rules.md says copies of a click are identical, so this is equivalent, and identical copies land in the same person partition, where they are dropped as neighbours.
- IDs of at most 15 bytes are packed into exact u128 keys (numeric order = byte order); longer IDs are interned under a mutex. Hashes only place keys; identity is always the full key.
- Input is read with positional reads into reused per-thread buffers, not mmap (see Phase 3).
- Dockerfile: `rust:1` builder, `debian:stable-slim` runtime, binary statically linked against glibc (`+crt-static`, explicit `--target`), `target-cpu=x86-64-v3` (not v4: AVX-512 frequency drops on Cascade Lake, and the hot code is AVX2). Floating tags because they always exist; the static binary does not depend on the runtime image's libc. Debian (not distroless) so `scripts/docker_run.sh`'s `chmod` cleanup works.
- Dockerfile review against the scorer: CMD has no arguments; runs as `USER 1000:1000`; writes only under `$OUTPUT_DIR`; never touches `/tmp` or `$HOME`; no network at run time (crates fetched at build time, `--locked`; only memchr and mimalloc); threads = `$NCPU` (8), far under 4,096; nothing on stdout (a few timing lines on stderr).
- mimalloc: `purge_delay = -1` (freed memory is kept for reuse). Transparent huge pages left at the system default (mimalloc keeps THP allowed); `allow_large_os_pages` deliberately not enabled: it would `madvise(MADV_HUGEPAGE)`, which under the common `defrag=madvise` setting can stall faults on direct compaction, and it cannot be measured here.

## Phase status
### Phase 1: reference (done)
- `reference/reference.py`: plain Python + pyarrow, hand-written CSV parser.
- Fixtures: 36/36. Dev set: exact MATCH with dev-answer (117,038 user_daily rows, 27,400 campaign_daily rows). About 38 s.

### Phase 2: Rust pipeline (done, tag v1)
1. Fixtures 36/36.
2. Dev set: exact MATCH with dev-answer.
3. Differential (`scripts/diff_test.py`): seeds 2-12 at 200k-2M events, seeds 15 and 249 whose windows hold 2028-02-29 (249's last day is Feb 29): ALL MATCH. Since v7 also two long-ID variants (15 datasets).
4. Docker: not available locally; Dockerfile reviewed (see Decisions).
5. v1 timings: 20M 7.4-7.8 s (peak private 2.5 GiB); 60M 44-47 s (peak private 7.6 GiB, partly disk-bound: the 4.7 GB input does not stay cached next to the heap on a 16 GB laptop).

### Phase 3: speed (in progress; v7 is the current verified best)
- v2: Windows first-touch page faults of memory-mapped input cost ~2.5 s for 1.6 GB (the scan itself 0.08 s); positional reads into reused buffers ~0.6 s, so pread was adopted. AVX2 tokenizer (bit masks of `,` `\n` `"` per 64-byte block; quote validity by counting), fuzzed against the plain splitter (caught an unterminated last line ending in an empty field). Event dedup by per-partition hash table with tombstones instead of sorting 64-byte records; link maps pre-sized; mimalloc.
- v3: counting-sort grouping by dense person id; hand-written Parquet writer (drops the parquet crate; pyarrow/DuckDB cross-checked incl. multi-row-group and empty files); product/campaign maps keyed by packed IDs.
- v4: timestamp fast path (all digits at once, month-start table, 8-digit SWAR for F3; 400k-input equivalence test); partition buffers pre-sized from input bytes. -15% vs v3.
- v5: mimalloc purge disabled (kernel CPU -1.5 s per 20M run); grouping with a small custom table, ids stored in the records, no zero-fill.
- v6: the person lookup was the largest single parse cost (skipping it: 19 s -> 10 s single-threaded). Now a flat (key, name) table (one cache miss per hit) and rows resolved in batches of 32 with software prefetch (filter word, then slot). 3.05 -> 2.38 s, CPU 15.7 -> 11.8 s. Partitions processed largest-first (whale persons).
- v7: parallel union-find (CAS linking of the larger-key root under the smaller, path halving), so every root is its set's smallest ID and naming is a parallel pass; stress-tested against a sequential union-find (30/30 runs). Links -30%. Branchless unquoting.
- Scale cross-check (v7 vs v1, independent implementations): exact match on 20M (2,245,672 user_daily rows) and 60M (6,614,475 rows). 60M with v7: 16.0 s on the loaded laptop, peak private 5.8 GiB.
- Latest profile (20M, 8 threads, throttled laptop): events parse 2.85 s (incl. Windows read overhead), sessionize 1.05, clicks 0.48, links 0.33, dedup 0.27, write 0.10.

## Timings per tag (20M seed 101, local laptop)
| Tag | 20M | 60M | Notes |
| --- | --- | --- | --- |
| v1 | 7.8 s | 45.6 s | correct baseline: mmap, sort-based dedup, 64-byte records, parquet crate |
| v2 | 4.5 s* | - | pread, AVX2 tokenizer, hash dedup with tombstones, partitioned link table, mimalloc |
| v3 | ~4.7 s* | ~26 s | dense-id grouping, hand-written Parquet writer, packed-key lookups |
| v4 | v3 -15%* | - | timestamp fast path, pre-sized partition buffers |
| v5 | 3.85 s* | - | mimalloc purge off, faster grouping |
| v6 | 2.38 s* | - | flat link table, batched prefetching person lookups, largest partitions first |
| v7 | 5.4 s** | 16.0 s* | parallel union-find, branchless unquote (v1 15.6 s in the same session) |

\* under another user's CPU-heavy job; \*\* on battery, throttled. Compare rows only through the A/B notes above.

## Tried and rejected (measured, no gain)
- Partition count 512 instead of 256, read chunks of 1 MB or 4 MB instead of 2 MB: all within noise.
- Sorting each person's events by packed u64 keys (ts << 24 | index) instead of the 32-byte records, fused into the person loop: same total time.

## Open ideas
- Tokenizer: per-line comma extraction with popcount (no per-separator branch); likely 5% of total.
- Sessionize attribution: split the per-thread campaign counters (credited vs assisted) for better cache locality; skip the by-campaign sort for one-click persons.
- Smaller dedup copies (24 bytes): ingest as an offset, a narrower location.
