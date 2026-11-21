READY TO SUBMIT: v7 de79cc3f8b28063805c318615267fece3a937d12 (local 20M: 5.4 s on a throttled, battery-powered laptop where v1 takes 15.6 s; about 2.4 s for the near-identical v6 on mains power)

# Progress

## Environment (Windows 11 laptop: i5-1135G7, 4 cores / 8 threads, AVX-512, 16 GB RAM)
- No WSL distribution installed, no Docker, no GitHub CLI. Per instructions none of these were installed.
  - The Docker image cannot be built or run locally; the Dockerfile is reviewed by hand against the scorer environment (below).
  - The repo is committed locally only. **It still needs a public GitHub repo (buildathon-shopping-sessions) and a push, including tags** (`git push -u origin main --tags`).
- Rust: rustup via winget (stable 1.100, x86_64-pc-windows-msvc). Visual Studio Build Tools 2026 were already present, so the MSVC linker works. The `x86_64-unknown-linux-gnu` std is installed: with mimalloc temporarily removed, `cargo check --target x86_64-unknown-linux-gnu` with the Dockerfile's RUSTFLAGS type-checks all of our Linux code paths (mimalloc's C part needs the image's gcc).
- Python 3.14.3 with pyarrow, polars, duckdb and psutil (pip --user).
- git 2.51 with global core.autocrlf=true; `.gitattributes` (`* -text`) overrides it for this repo.
- Datasets live in `../data/` (next to the repo, outside it):
  - `dev/` seed 1, 1M events (the public dev set); `diff/` the differential datasets and cached reference outputs;
  - `s101-20m/` seed 101, 20M events, 5M clicks; `s102-60m/` seed 102, 60M events, 15M clicks;
  - `bins/` saved binaries of earlier versions for A/B timing.
- Generator built from `../generator` (stale Cargo.lock deleted): `../generator/target/release/generate.exe`.

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
