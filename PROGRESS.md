READY TO SUBMIT: v3 cd0ce1e9eb045ec0f499090a2da63513b0e7381e (local 20M: about 4.5-4.9 s under heavy outside CPU load; v1 7.5 s under the same load; 60M about 26 s, disk-bound on this laptop)

# Progress

## Environment (Windows 11 laptop: i5-1135G7, 4 cores / 8 threads, AVX-512, 16 GB RAM)
- No WSL distribution installed, no Docker, no GitHub CLI. Per instructions none of these were installed.
  - The Docker image cannot be built or run locally; the Dockerfile is reviewed by hand against the scorer environment (below).
  - The repo is committed locally only. **It still needs a public GitHub repo (buildathon-shopping-sessions) and a push, including tags** (`git push -u origin main --tags`).
- Rust: rustup via winget (stable 1.100, x86_64-pc-windows-msvc). Visual Studio Build Tools 2026 were already present, so the MSVC linker works. The `x86_64-unknown-linux-gnu` std is installed for `cargo check --target x86_64-unknown-linux-gnu` (type-checks the Linux build; it cannot link or run here).
- Python 3.14.3 with pyarrow, polars and psutil installed via pip --user.
- git 2.51 with global core.autocrlf=true; `.gitattributes` (`* -text`) overrides it for this repo.
- Datasets live in `../data/` (next to the repo, outside it):
  - `dev/` seed 1, 1M events (the public dev set); `diff/` the differential datasets and cached reference outputs;
  - `s101-20m/` seed 101, 20M events, 5M clicks; `s102-60m/` seed 102, 60M events, 15M clicks.
- Generator built from `../generator` (stale Cargo.lock deleted): `../generator/target/release/generate.exe`.

## Decisions
- Column order is taken as fixed by rules.md (the generator always writes the same header); the first line of each file is skipped as the header whatever it contains (fixture L3 shows a later identical line is data).
- Input files: every regular file in the folder whose name ends `.csv` and does not start with `.`.
- `ingest_ts` is always valid F1 per rules.md; should one fail to parse, that copy sorts after every valid copy in dedup (and is then late, so dropped).
- Years in F1/F2 are exactly 4 ASCII digits; year 0000 is treated as bad (it is outside every window anyway).
- products.csv / campaigns.csv rows with an empty field are skipped; a repeated ID keeps its last row (both implementations).
- Clicks are validated before dedup: rules.md says copies of a click are identical, so this is equivalent.
- IDs of at most 15 bytes are packed into exact u128 keys (numeric order = byte order); longer IDs are interned under a mutex (never seen in generated data, but handled).
- Dockerfile: `rust:1` builder, `debian:stable-slim` runtime, binary statically linked against glibc (`+crt-static`, explicit `--target`), `target-cpu=x86-64-v3`. Floating tags chosen because they always exist; the static binary does not depend on the runtime image's libc. Debian (not distroless) so `scripts/docker_run.sh`'s `chmod` cleanup works.
- Dockerfile review against the scorer: CMD has no arguments; runs as `USER 1000:1000`; writes only under `$OUTPUT_DIR`; never touches `/tmp` or `$HOME`; no network at run time (crates are fetched at build time, `--locked`); threads = `$NCPU` (8), far under 4,096; nothing on stdout (logs go to stderr).

## Phase status
### Phase 1: reference (done)
- `reference/reference.py`: plain Python + pyarrow, hand-written CSV parser.
- Fixtures: 36/36. Dev set: exact MATCH with dev-answer (117,038 user_daily rows, 27,400 campaign_daily rows). About 38 s.

### Phase 2: Rust pipeline (done, tag v1)
1. Fixtures 36/36 (`python scripts/run_fixtures.py fixtures -- target/release/shopping-sessions.exe`).
2. Dev set: exact MATCH with dev-answer.
3. Differential: `scripts/diff_test.py`, 13 datasets (seeds 2-12 at 200k-2M events, seeds 15 and 249 whose windows hold 2028-02-29; 249's last day is Feb 29): ALL MATCH.
   Plus unit tests: timestamp/price forms, CSV dialect, and a randomized brute-force check of credit and assists (3,000 cases).
4. Docker: not available locally; Dockerfile reviewed (see Decisions).
5. Local timings (this laptop, warm-ish cache, NCPU unset = 8 threads):
   - 20M (seed 101): 7.4-7.8 s, peak private 2.5 GiB, peak working set 2.9 GiB.
   - 60M (seed 102): 44-47 s, peak private 7.6 GiB, peak working set 8.0 GiB. The 4.7 GB input does not stay cached next to the heap on a 16 GB laptop, so this run is partly disk-bound.

### Phase 3: speed (in progress)
- Finding: on Windows, first-touch page faults of memory-mapped input cost ~2.5 s for 1.6 GB (scan itself 0.08 s); positional reads into a reused per-thread buffer take ~0.6 s. Linux faults are far cheaper, but pread is predictable on both and was adopted.
- Finding: link maps grew by rehashing (0.5 s); replaced by a partitioned open-addressing table built in parallel, plus a bitset filter in front of the person lookup.
- v3: hand-written Parquet writer (drops the parquet crate; Docker build much lighter). `scripts/check_parquet.py` reads output with pyarrow and DuckDB and checks schema (String, Date logical types), multi-row-group files (tested with 1,000-row groups) and empty outputs; all agree with polars.
- v3: 60M run peak private memory 7.1 GiB.
- v2: fixtures 36/36, dev MATCH, differential 13/13 MATCH, unit tests incl. 25k-round tokenizer fuzz against the plain splitter (it caught an unterminated last line ending in an empty field).

## Timings per tag (20M seed 101, local laptop)
| Tag | 20M median | 60M | Notes |
| --- | --- | --- | --- |
| v1 | 7.8 s | 45.6 s | correct baseline: sort-based dedup, 64-byte records, parquet crate |
| v2 | 4.5 s* | - | pread into reused buffers, AVX2 tokenizer, hash dedup with tombstones, partitioned link table, mimalloc |

| v3 | ~4.7 s* | ~26 s | counting-sort grouping by dense person id, hand-written Parquet writer, packed-key lookups |

\* From v2 on, another user's 8-process Python job shares this laptop's CPU, so absolute times are inflated and noisy; versions are compared A/B interleaved under the same load (v1 measured 7.3-7.7 s alongside v2).

## Open ideas
- Parse: SIMD bitmask scan of `,` `"` `\n` per 64-byte block instead of per-line memchr.
- Smaller records (invalid rows need only event_id + ingest_ts), hash-based dedup instead of sorting.
- Person lookup per partition rather than random probes into one big map.
- Faster allocator (mimalloc), pre-sized buffers, hand-written Parquet writer.
