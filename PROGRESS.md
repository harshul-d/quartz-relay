READY TO SUBMIT: v11 2394d7b51fbcdd00214b3e8741853f2f06116f1c (Actions, 20M, same job, warm container median v11 vs v10: 1.685 s vs 2.012 on EPYC 7763, 1.343 vs 1.590 on Xeon 6975P, 1.069 vs 1.327 on EPYC 9V45; CPU-s 5.71 vs 7.10, 4.69 vs 5.72, 3.50 vs 4.76; v10 scored 1.54 s and v8 1.39 s on the scorer)

# Progress

## Report: v11, the fast path for generator-format IDs
**Tagged v11** (same code as a7fbed1; benchmarked in run a7fbed1 against v10 in the same job, 20M, 4 vCPUs):

| Runner | CPU-s v10 -> v11 (native, user+sys) | Container warm wall | Container cold wall |
| --- | --- | --- | --- |
| EPYC 7763 | 7.10 -> **5.71** (-20%) | 2.012 -> **1.685** (-16%) | 5.119 -> 4.992 |
| Xeon 6975P | 5.72 -> **4.69** (-18%) | 1.590 -> **1.343** (-16%) | 3.494 -> 3.329 |
| EPYC 9V45 | 4.76 -> **3.50** (-26%) | 1.327 -> **1.069** (-19%) | 5.107 -> 5.001 |

The first fast-path build (F1) alone was -15 to -17% CPU in an earlier job; the refinements in F2 add 3-6%. Cold runs are dominated by reading 1.9 GB from disk and barely move.

**How much took the fast path** (20M, seed 101): 21.19M event rows; only 123 event IDs and 106 valid rows' user IDs did not decode. Of 19.55M surviving events, 17.61M (90%) were finished on the fast path and 1.95M went to the general path: the five whales (several shards each), shared-device people, people at a shard edge, people whose events another person's revised copies interrupt, and long alias chains. Clicks: 4.83M of 5.11M valid (95%) fast. At 60M: 51.9M of 58.7M events (88%), 14.4M of 15.3M clicks (94%).

**Correctness** (all on this code): unit tests in release and debug mode (14, including scramble round trip, encoder against the generator's code, SWAR decoder against the scalar one, synthetic key recovery); key recovery against the generator's true keys on 22 seeds (20k-3M events) 22/22 exact; fixtures 36/36; dev answer MATCH (fast path on and off); differential suite 33/33 (25 seeds of different sizes, 2 long-ID, 6 adversarial: all user IDs remapped, a tenth remapped, added upper-case/space-padded variants; the last two mix both paths in one run); 20M and 60M exact match with v1; Linux fixtures and dev answer in all three Actions shards; pyarrow/DuckDB read the output identically. Peak private memory: 1.65 GiB at 20M, 4.7 GiB at 60M (v1: 2.5 / 7.6 GiB; F1 measured 1.37 / 3.43 before link classification moved alongside event parsing). If key recovery fails or under half the IDs decode, the whole run takes the general path, which measured faster than v10 (`general` variant: 5.55 vs 5.71, 6.56 vs 6.72, 5.52 vs 5.68 CPU-s).

**Next, ranked:**
1. Event parsing is now about half of all CPU: tokenizer ~15%, timestamps and ID decoding ~10%, the 20-byte scatter ~4%. A line splitter that cuts all 7 fields from one 64-byte mask (no per-separator walk) and an F1 timestamp parser in vector registers are the remaining large items.
2. Fewer general-path people: whales and shard-edge people could be finished across neighbouring shards (a second pass over runs that touch a shard edge) instead of hashing them; worth ~10% of events.
3. 8-byte keys for the general path (now ~10% of events), and 24-byte local click records (click_id as its scrambled number, byte order compared only on ts ties).
4. Memory: peak rose with the overlapped link classification; if the scorer's memory limit is tight, run it before event parsing (costs ~0.03 s).

## Report (structural phase, written after the fused-parse rewrite was benchmarked)
**Where we are.** Tags v9 and v10 are verified wins on Actions; v10 is READY. Since then: no new tag (every candidate was under 20%, which the scorer's ±20% machine spread cannot confirm; v10 scored 1.54 s vs v8's 1.39 s there). HEAD = v10 + parallel link filter + hash reuse + unchecked unquoting, about 3% less CPU than v10.

**CPU and wall per version** (Actions, 20M, run 0a8db5f, all four in the same job; native warm under `/usr/bin/time`, 4 threads):

| Runner | | v8 | v9 | v10 | HEAD |
| --- | --- | --- | --- | --- | --- |
| EPYC 7763 | CPU-s (user+sys) | 7.23 | 6.95 | 6.62 | 6.44 |
| | container warm wall | 2.091 | 1.968 | 1.898 | 1.831 |
| EPYC 7763 | CPU-s | 7.43 | 7.14 | 6.82 | 6.64 |
| | container warm wall | 2.128 | 2.006 | 1.940 | 1.880 |
| EPYC 9V74 | CPU-s | 6.31 | 6.01 | 5.74 | 5.66 |
| | container warm wall | 2.138 | 1.822 | 1.678 | 1.786 (noisy) |

Single-thread CPU (EPYC 7763): v8 4.76 s, HEAD 4.25 s (-11%). The budget for 0.30 s on 8 vCPUs is about 2 CPU-s: we are at 2.1-2.4x that even single-threaded, and hyperthreads inflate it a further ~1.5x.

**What scaled** (native, 1 -> 2 -> 4 threads on 2 cores x 2 hyperthreads; HEAD, EPYC 7763): every phase scales ~1.9x from 1 to 2 threads (two cores) and ~1.3x more on hyperthreads. Events parse 2.18 / 1.12 / 0.89 s (2.45x), sessionize 1.29 / 0.66 / 0.45 (2.86x), clicks 0.38 / 0.19 / 0.15 (2.6x), dedup 0.19 / 0.10 / 0.07 (2.7x), ID linking 0.19 / 0.11 / 0.09 (2.1x, the worst), lookups 0.011 (serial but tiny). No phase is serial enough to matter on 4 cores: the gap is total work, not scaling.

**What did not work** (measured): mmap in four variants (no wall gain, -0.1 CPU-s of sys), dedup/clicks overlap (worse), streaming non-temporal partition writes (no gain), a flattening tokenizer (slower), and the fused tokenize+decode parse for events and clicks (slightly slower; per-field dispatch costs what the span arrays did). The tokenizer itself already runs at ~1.8 GB/s per physical core; the earlier "250 MB/s" note was a miscalculation (corrected below).

**Why the teammate pipeline is faster:** see the next section. In short, it recognizes the generator's ID encoding, turns IDs into dense integers, and so writes each event once (20-byte records), dedups with an array and groups people without hashing, which removes about two of our full passes over memory and nearly all hashing. Its general path is structurally like ours.

**Next, ranked:**
1. **Dense-ID fast path** (teammate idea 1): recover the generator's per-seed user and event scramble keys from data pairs, decode IDs to integers, shard copies by event number (array dedup, L2-resident), finish people whose events all land in one shard without hashing, route everyone else (whales, shared devices, cross-shard people, IDs that do not decode) to today's general path. Exact for any input. Expected: the largest CPU cut available (the teammate is ~30% faster on the scorer with it). Cost: a large change (key recovery with tests against the generator's known keys, a second pipeline, fallbacks); it relies on the generator's internal ID scheme, which is a deliberate choice to make.
2. **8-byte keys** (teammate idea 2): IDs of up to 9 ASCII bytes packed 7 bits per byte into a `u64`, `u128` as a whole-run fallback. Records shrink 32 -> 24 bytes (events, copies) and 48 -> 24-32 (clicks); hashes and compares halve. Expected ~10-15% CPU; generic, no data assumptions.
3. **PLAIN output again**: the scorer's disk looks fast (the teammate writes 90 MB of PLAIN Parquet and scores 1.04 s), and our dictionary encoding costs ~0.1 CPU-s single-threaded (sessionize 1.16 -> 1.29 s since v8, output 0.08 -> 0.00). Keep it only if the scorer's disk is slow.
4. **Sessionize** (29% of CPU): a dense-id path (1) removes the grouping hash and counting sort; without it, the per-person sort of 32-byte records (~5% of CPU) and attribution's `counts` cache misses are next.

## Comparison with the teammate pipeline (scorer: 1.04 s; ours v8 1.39 s, v10 1.54 s)
Read in full from a clone outside this repo (`../teammate/`). Scorer numbers differ by up to ±20% per machine, so v10's Actions gains (under 20%) are unproven on the scorer. Differences, ranked by likely impact:

1. **A generator-shaped fast path (its largest file).** The generator writes every user and event ID as a letter plus canonical base 36 of a 40-bit bijective scramble with two fixed multipliers and a per-seed xor key. The teammate pipeline recovers both keys from a sample (two linked IDs of one person, or two events of one person, have nearby indexes; the key bits are solved from such pairs and scored), then decodes IDs back to dense integers. Consequences: (a) records are 20 bytes of `u32`s (event number, ingest seconds, ts offset, dense user, price | category); (b) every copy is written **once**, into its event-number shard (2^16 events per shard), so there is no second scatter by person; (c) dedup is an array indexed by event number (L2-resident), with no hashing; (d) a person's events have consecutive numbers, so after dedup each person is one run in one shard: no grouping hash table, no counting sort; people who span shards, whales, shared devices and any ID that does not decode go to the general path. Output stays exact for any input (a wrong key or foreign IDs only route rows to the general path). This removes roughly two of our full passes over memory (the person scatter and the sessionize regroup) and nearly all hashing. **Likely most of the gap.** Not adopted yet: a large port (~1,300 lines there), and it relies on the generator's internal ID encoding, which the rules do not forbid (nothing is hard-coded and the result is exact) but which is a deliberate choice; it is the top item in the plan below.
2. **8-byte keys in the general path.** IDs of up to 9 ASCII bytes are packed 7 bits per byte into a `u64` (order-preserving); `u128` and byte-slice keys are whole-run fallbacks. Payload records are 24 bytes (ours 32), dedup copies 24 (ours 32), clicks 24-32 (ours 48); hashes and compares are on one word.
3. **mmap input with `MADV_WILLNEED`, per-chunk `MADV_POPULATE_READ`**, chunks of total/(threads x 32) clamped to 256 KB-16 MB. Our Actions A/B (H21) found no wall gain for mmap over pread, so this is not the gap.
4. **Line splitter for events/clicks** that loads 64 (or 128) bytes from the line start and cuts the whole line from its masks with one popcount of commas (lines are < 128 bytes), plus F1 timestamps in one 32-byte AVX2 load (digits, separators and range checks in vector registers), both IDs base-36-decoded in one SSE register, `p<digits>` products and `c_<digits>` campaigns looked up in arrays. Ours: per-separator walk, date cache + SWAR time, hashed product/campaign maps.
5. **Person lookups issued 16 rows ahead** through a ring (ours: batches of 32). Similar.
6. **Exit:** after writing, `MADV_DONTNEED` on the input mappings from all threads, then `process::exit` (the kernel's exit teardown is single-threaded). Ours uses pread (no input mappings); our measured exit is 0.04-0.05 s on fast disks.
7. **Output:** PLAIN Parquet, no dictionaries, one file per thread (about 90 MB at 20M). Ours dictionary-encoded, 20 MB. Since the teammate scores 1.04 s with 90 MB, the scorer's disk is probably fast, so our output work (H8/H9) buys little there.
8. **Dockerfile / runtime:** `FROM scratch` with only the static binary (ours `debian:stable-slim`), `MIMALLOC_PURGE_DELAY=-1` by environment (ours by API, same effect), `rust:1.85-bookworm`, the same `target-cpu=x86-64-v3` and `+crt-static`, rayon thread pool of `NCPU` threads. Release profile identical (fat LTO, 1 codegen unit, panic abort). Container start in our measurements is ~0.1 s either way; minor.
9. **Same as ours:** 256 hash shards, parallel CAS union-find with the smallest ID as name, dedup by smallest ingest with tombstones in place, counting sort per shard in the general path, the same attribution (two pointers for credit, merged `[t, t+7d]` intervals for assists).

## Environment (Windows 11 laptop: i5-1135G7, 4 cores / 8 threads, AVX-512, 16 GB RAM)
- No WSL distribution installed, no Docker, no GitHub CLI. Per instructions none of these were installed.
  - The Docker image cannot be built or run locally; the Dockerfile is reviewed by hand against the scorer environment (below).
  - The repo is public at github.com/harshul-d/quartz-relay (origin; pushes work with stored credentials). Branches: `main` (tagged versions), `perf` (experiments, triggers the benchmark), `bench` (benchmark results only).
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
- Runs on pushes to `perf` and on manual dispatch, on `ubuntu-latest` (4 vCPU = 2 cores x 2 threads; the CPU model changes from run to run: EPYC 7763 / 9V74 / 9V45, Xeon 6975P / 6973P / 8573C seen). Three identical shards per push (matrix), so one push covers several machine types; compare variants only within one shard. Results: `results/<sha>-<shard>.md` on branch `bench` (`git fetch origin bench; git show origin/bench:results/<sha>-1.md`), also in each job's step summary and as notice annotations. Shard 1 also profiles HEAD natively with `perf` (flat profile plus the hottest source lines with their text, via `ci/srclines.py`). Poll a run without login: `GET https://api.github.com/repos/harshul-d/quartz-relay/actions/runs?head_sha=<sha>`.
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

### v9 (9a99efd): key packing from the chunk + dictionary-encoded user_daily + read-ahead hint
Same-job comparison, 20M, container wall medians of 5 (min-max), run 9a99efd:

| Runner | v8 warm | v9 warm | v8 cold | v9 cold | v9 exit after main (warm) |
| --- | --- | --- | --- | --- | --- |
| EPYC 9V74 (slow disk) | 2.130 (2.106-2.136) | **1.780** (1.760-1.836) | 5.643 | 5.264 | 0.121 s (v8-era code: ~0.43) |
| EPYC 9V45 (slow disk) | 2.103 (2.075-2.134) | **1.738** (1.664-1.760) | 5.742 | 5.426 | 0.121 s |
| Xeon 6975P (fast disk) | 1.730 (1.709-1.736) | **1.659** (1.652-1.666) | 3.581 | 4.035 (3.54-4.31, bimodal on both) | 0.036 s |

v9 phase breakdown (EPYC 9V74, container warm): lookups 0.008, ID linking 0.089, events read+parse 0.849, dedup 0.063, clicks 0.134, sessionize 0.358 (cpu-s: group events 0.61, group clicks 0.26, sessions 0.16, attribution 0.40), output writing 0.043, start 0.098, exit 0.121. Output 20.3 MB (v8: 93.3 MB). Linux fixtures and dev answer pass in all 3 shards; 20M output matches v8 in all 3; locally fixtures 36/36, dev MATCH, differential 15/15, pyarrow/DuckDB check OK, 20M/60M exact match with v1.

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
| H5b' | (same, 582bb02, 3 shards) | Shard 1 (EPYC 7763): everything exits in ~0.05 s. Shards 2 and 3 (slow-disk runners): head exit 0.435 / 0.453 s warm (1.79 s cold on shard 3); with `PIPELINE_FSYNC=1` exit 0.05 s and output writing 0.42-0.48 s instead; a probe that only writes 93 MB: exit 0.43 s, or 0.05 s + 0.49 s in main with fsync; probes touching 2 GB or reading all 1.9 GB input: exit 0.05 s | **Confirmed: `docker run` returns only once the container's dirty output is on disk** (the overlay unmount syncs the host filesystem). Cost ~4.6 ms per MB of output on those runners, cold or warm; every pipeline writing ~90 MB of PLAIN Parquet pays ~0.4 s. Fix: write fewer bytes |
| H7 | Parse: the largest single line in the events/clicks callbacks is `bswap` in `u128::from_be_bytes` of `key::pack_short` (21% / 31% of those symbols): a store-forwarding stall (variable-length memcpy into a stack buffer, then one 16-byte load) | Fixed with `Interner::key_in(chunk, field)`: one 16-byte load from the chunk, masked to the field length (unit test against `pack_short` for every offset/length incl. chunk ends) | Kept (v9). ff86f5e, same job: events parse 1.079 -> 1.020 s, clicks 0.188 -> 0.163 (EPYC 7763); warm -2 to -3% on all three shards |
| H8 | Output bytes: user_daily dictionary-encoded (dictionary page PLAIN + data page RLE/bit-packed indices, whichever is smaller; user_id one entry per person run, no hashing) | 20M output 93.3 MB -> 20.3 MB, 60M 272.8 -> 57.5 MB; exact match with v1 on 20M/60M; pyarrow/DuckDB/polars identical; `PIPELINE_PLAIN_OUTPUT=1` restores the old layout for A/B. ff86f5e, same job vs PLAIN: Xeon 8573C (slow disk) warm 2.277 -> 1.998 s, cold 5.643 -> 5.380; fast-disk runners -0.5 to -1% | Kept (v9) |

### v10 (fa8f1c1): v9 + early user_daily writes with writeback (H9) + cached date parsing (H10)
Same job (run fa8f1c1), 20M, container wall medians of 5 (min-max):

| Runner | v8 warm | v9 warm | v10 warm | v8 cold | v10 cold |
| --- | --- | --- | --- | --- | --- |
| EPYC 7763 | 2.075 (2.037-2.209) | 1.983 | **1.906** (1.885-1.967) | 5.535 | 5.135 |
| Xeon 8573C (slow disk) | 2.684 (2.675-2.803) | 2.333 | **2.156** (2.145-2.166) | 9.354 | 8.899 |
| EPYC 9V45 | 1.746 (1.730-1.796) | 1.430 | **1.276** (1.270-1.341) | 5.475 | 5.067 |

v10 phases, container warm (EPYC 7763 / Xeon 8573C / EPYC 9V45): lookups 0.011 / 0.012 / 0.007; ID linking 0.104 / 0.115 / 0.078; events read+parse 0.935 / 1.050 / 0.611; dedup 0.072 / 0.117 / 0.053; clicks 0.150 / 0.159 / 0.097; sessionize incl. most output encoding 0.455 / 0.533 / 0.312 (cpu-s: group events 0.70 / 0.86 / 0.47, group clicks 0.29 / 0.33 / 0.19, sessions 0.16 / 0.20 / 0.10, attribution 0.46 / 0.51 / 0.34); output writing at the end 0.005 / 0.003 / 0.003; start 0.122 / 0.111 / 0.080; exit 0.052 / 0.050 / 0.038. Linux fixtures and dev answer pass in all 3 shards; 20M output matches v8 in all 3; locally fixtures 36/36, dev MATCH, differential 15/15, 20M exact match with v1, pyarrow/DuckDB check OK.

### After v9
| # | Change | Result (same-job warm medians) | Decision |
| --- | --- | --- | --- |
| H9 | Write user_daily while sessionizing (a file per 65,536 rows per thread) and start each file's writeback at once (`sync_file_range(SYNC_FILE_RANGE_WRITE)`); `PIPELINE_EARLY_WRITE=0` restores write-at-end | 014e4ab: Xeon 8573C (slow disk) 2.062 -> 1.987 s, exit 0.132 -> 0.041; EPYC 7763 1.992 -> 1.987; Xeon 6975P 1.677 -> 1.669. cd25d12: EPYC 9V74 1.754 -> 1.664, exit 0.118 -> 0.041. Encoding now runs inside sessionize (+0.04 s there, -0.045 s at the end) | Kept |
| H10 | Timestamps: per-thread cache of parsed dates keyed by the exact 10 date bytes (a window has a few dozen dates), SWAR `HH:MM:SS`; fuzz-tested (400k cases, cold and warm cache) against the plain parser | cd25d12 vs 014e4ab: events parse 0.986 -> 0.926 / 0.844 -> 0.792 / 0.838 -> 0.776; warm 1.946 -> 1.873 (EPYC 7763), 1.664 -> 1.619 (EPYC 9V74), 1.718 -> 1.641 (Xeon 6975P) | Kept |
| H11 | Output encoding: dictionary keys within 4,096 of a base (days, counts, zero revenue, categories) through a direct table instead of hashing | 22fc20e vs cd25d12: EPYC 7763 1.913 -> 1.903 (spreads overlap), Xeon 8573C 2.206 -> 2.202, EPYC 9V45 1.268 -> 1.311 | Rejected (no clear gain), reverted |
| H12 | Partition count 128 / 512 / 1024 vs 256 (branches `exp-p7`, `exp-p9`, `exp-p10`) | 7f6a083 (warm; events / dedup / sessionize): EPYC 9V45 p256 1.449, p128 1.497, p512 1.425, p1024 1.404 (spreads overlap; p128 dedup 0.111 vs 0.059); EPYC 7763 1.906 / 1.927 / 1.887 / 1.882 (overlap); EPYC 9V74 noisy (1.637 / 1.864 / 1.759 / 1.645) | Rejected: no clear gain, 256 stays |

### After v10
| # | Change | Result (same-job warm medians) | Decision |
| --- | --- | --- | --- |
| H13 | ID linking: count linked IDs per partition and fill the bit filter (atomic OR) in parallel instead of two sequential loops | 5fe3eff: ID linking 0.093 -> 0.087 / 0.090 -> 0.090 / 0.095 -> 0.093; wall within spread | No measurable effect; kept provisionally (strictly less sequential work), judged with the next set |
| H14 | Flush: an unlinked user's partition from the hash already computed for the filter (saves one 128-bit hash per row) | 5fe3eff (with H13): EPYC 7763 1.879 -> 1.862 (overlap), Xeon 6975P 1.594 -> 1.582 (beyond spread), EPYC 7763 1.888 -> 1.897 (overlap) | Same: kept provisionally |
| C11 | Candidate set H13+H14+H17 vs v8/v10, same job (f9ba71f) | warm: EPYC 9V45 v8 1.937, v10 1.486, head 1.459 (spreads overlap); EPYC 9V74 2.095 / 1.606 / 1.573 (overlap); Xeon 8573C noisy (container start 0.34 s) 2.564 / 2.122 / 2.240 | Not tagged: not clearly beyond spread in this run (b0149c8 had it beyond spread on one runner). v10 stays READY |
| H20 | Dedup and clicks parsing side by side (two thread groups sharing the CPUs) | 3cfd50d, native wall / CPU-s (head vs head-noovl): 1.758 / 6.76 vs 1.764 / 6.63; 1.461 / 5.64 vs 1.472 / 5.55; 1.689 / 6.48 vs 1.654 / 6.39; container warm worse on all 3 shards (1.922 vs 1.895, 1.615 vs 1.581, 1.845 vs 1.826) | Rejected (oversubscription), reverted |
| H21 | Linux mmap instead of pread (warm): plain, MAP_POPULATE, MADV_SEQUENTIAL, MADV_POPULATE_READ per chunk in the parsing worker (`PIPELINE_READ`) | 3cfd50d, container warm (pread / mmap / populate / seq / chunk): EPYC 7763 1.922 / 1.935 / 1.976 / 1.961 / 1.937; EPYC 9V74 1.615 / 1.669 / 1.765 / 1.711 / 1.691; EPYC 7763 1.845 / 1.867 / 1.915 / 1.895 / 1.872. CPU-s (native): 6.76 / 6.63 / 6.69 / 6.60 / 6.77 (sys 0.53 -> 0.42 with mmap). MAP_POPULATE cold +0.8 s | Rejected: no wall gain (mmap saves ~0.1 CPU-s of sys but page-cache faults cost about what the copy did); pread stays. Code kept behind the switch |
| D2 | Thread scaling of v10, native warm, 1 / 2 / 4 threads (3cfd50d, EPYC 7763) | events 2.284 / 1.165 / 0.931 (2.45x); sessionize 1.292 / 0.662 / 0.454 (2.85x); clicks 0.398 / 0.200 / 0.151; dedup 0.194 / 0.106 / 0.074; ID linking 0.205 / 0.118 / 0.101 (2.0x, the worst); lookups 0.012 / 0.012 / 0.011 (serial, tiny); wall 4.397 / 2.271 / 1.739; CPU-s 4.39 / 4.49 / 6.71. 1 -> 2 threads (2 cores) ~1.95x, 2 -> 4 (hyperthreads) ~1.3x. Same shape on EPYC 9V74 | No large serial phase. The gap is total work: ~4.4 single-thread CPU-s (events parse 52%, sessionize 29%) vs a budget of ~2 |
| H22 | Fused events parse: the tokenizer hands each unquoted field to a sink at its separator (`csv::for_each_field`); event_id key, both timestamps and the event type are decoded on the spot into locals; user/product/price stay slices (product and price decoded only for purchases); validation at the line end, unchanged. No span, value or argument arrays per line | Fuzz test vs `for_each_record` (23k inputs, N = 2/4/7); all local checks pass. 3bb5ea7 (vs `before` = same code without it), single-thread events parse: 2.019 -> 2.096 (EPYC 9V74), 2.297 -> 2.409 (EPYC 7763), 2.493 -> 2.526 (Xeon 8573C); CPU-s 5.64 -> 5.70, 6.69 -> 6.82, 7.70 -> 7.75 | Rejected, reverted: per-field dispatch costs what the span/value arrays did |
| H23 | The same fused decoding for clicks | ceeb9c6, single-thread clicks parse 0.376 -> 0.389, 0.381 -> 0.398, 0.427 -> 0.426; CPU-s (before / fused events / + fused clicks) 6.43 / 6.56 / 6.61, 6.58 / 6.66 / 6.76, 8.00 / 8.22 / 8.19 | Rejected, reverted |
| D1 | Cost attribution of events parse (diagnostic `PIPELINE_SKIP`: 1 = no dedup copies, 3 = no records at all, 4 = no lookup/scatter, 8 = nothing after tokenizing) | 952aeb4, events read+parse warm (EPYC 7763 / EPYC 9V45 / Xeon 8370C): full 0.938 / 0.711 / 1.266; no copies 0.872 / 0.623 / 1.171; no records 0.809 / 0.573 / 1.078; no lookup+scatter 0.750 / 0.485 / 0.948; tokenize only 0.418 / 0.292 / 0.609 (reads themselves ~0.04 wall). So: read+tokenize ~45%, row work (timestamps, keys, validation) ~32%, person lookup ~11%, each scatter ~7% | Read+tokenize is the largest block. (Correction: an earlier note here said ~250 MB/s per hyperthread; that was a miscalculation. Tokenize-only is 0.418 s for ~1.55 GB of event lines on 4 hyperthreads = ~3.7 GB/s aggregate, ~0.9 GB/s per hyperthread, ~1.8 GB/s per physical core; the whole events parse ~0.8 GB/s per core.) |
| H15 | Tokenizer: per 32 KB window, flatten separators (position, newline flag, running quote count) with no data-dependent branch, then check each line with one compare (N - 1 commas then `\n`) and cut fields from the positions; the old single loop branched on "newline?" at every separator | Tests: new multi-window fuzz test (40 inputs of 20-170 KB with long malformed lines, quotes, empty lines, with/without final newline) against the plain splitter and the old loop; all local checks pass. 549481c (tokenize-only, old -> new): Xeon 6975P 0.407 -> 0.481, Xeon 8573C 0.511 -> 0.594, EPYC 9V74 0.345 -> 0.416; full warm 1.622 -> 1.714 / 2.213 -> 2.329 / 1.648 -> 1.726 | Rejected (the three array stores per separator cost more than the mispredict they remove), reverted |
| H16 | Chunk size: a 2 MB chunk is copied into a buffer larger than L2 and then tokenized, so its start is re-read from L3/memory; 1 MB / 512 KB / 256 KB via `PIPELINE_CHUNK_KB` (full local suite also passes at 64 KB) | 88429c2, warm 2 MB / 1 MB / 512 KB / 256 KB: EPYC 9V74 1.645 / 1.654 / 1.667 / 1.673, Xeon 6975P 1.638 / 1.622 / 1.642 / 1.660, EPYC 7763 1.906 / 1.907 / 1.919 / 1.919; cold much worse when smaller: 5.24 / 5.57 / 5.72 / 6.25 (EPYC 9V74), 3.56 / 3.64 / 3.80 / 4.10 (Xeon 6975P) | Rejected; larger chunks next (cold) |
| H17 | `emit` without bounds checks (indices from the tokenizer's invariants, `debug_assert`ed; debug-mode tests added to `scripts/verify.ps1`, which also caught an overflow in a test's own input generator) | a863bec vs 88429c2: events parse 0.808 -> 0.774 / 1.083 -> 1.024 / 0.664 -> 0.636; warm 1.640 -> 1.618 (EPYC 9V74), 2.195 -> 2.123 (Xeon 8573C, beyond spread), 1.374 -> 1.359 (EPYC 9V45) | Kept |
| H18 | Chunks of 4 MB / 8 MB (cold reads) | a863bec: cold 4 MB vs 2 MB 8.851 vs 8.900 (Xeon 8573C, just beyond spread), 5.048 vs 5.109 (EPYC 9V45, overlap), EPYC 9V74 too noisy; warm neutral or worse (8 MB 2.180 vs 2.123) | Rejected: 2 MB stays |
| H19 | Partition scatter with write-combining: pair records per partition in an L1 staging slot and write each 64-byte pair with non-temporal stores (no read-for-ownership of ~1.3 GB); profile: the two scatter writes were 51% of `flush`, ~8% of all CPU. `PIPELINE_NT=0` = plain pushes | b0149c8, events parse streaming vs plain: EPYC 7763 0.926 vs 0.923, EPYC 9V74 0.746 vs 0.747, EPYC 9V45 0.548 vs 0.568 | Rejected (no clear gain), reverted. The set H13+H14+H17 (plain pushes) vs v10 in the same run: events parse 0.990 -> 0.923, 0.798 -> 0.747, 0.614 -> 0.568; warm 2.028 -> 1.930, 1.810 -> 1.767, 1.271 -> 1.238 (beyond spread) |

### Fast path for generator-format IDs (`src/fast.rs`, decided 2026-11-21)
Adopted the teammate pipeline's idea 1 in our own design. Source of truth for the ID scheme: `tools/gen/src/text.rs` (`scramble`, `base36`), `user.rs` and `plan.rs`; the teammate pipeline was only a second reference.

**Which IDs are encoded** (from the generator source):
- `user_id` = `u` + base 36 of `scramble(person * 512 + alias, user_key)`; shared devices use numbers from 2^39. Decoded.
- `event_id` = `e` + base 36 of `scramble(event_index, event_key)`; one person's events have consecutive indexes in time order. Decoded.
- `click_id` = `k` + base 36 of `scramble(click_index, click_key)`. Not decoded: clicks need the byte-wise click_id for the credit tie-break, copies already meet in their person's list, and there is no click dedup pass to remove.
- `product_id` = `p` + decimal, `campaign_id` = `c_` + decimal: plain numbers, no key. `src/direct.rs` answers both lookups by array index for canonical spellings (no leading zero) and falls back to the hash map for everything else; built from the finished maps, so first/last-row rules are unchanged. Removes a hash per click and per purchase.
- Keys: `scramble` is `((v * A) ^ key) & (2^40-1)`, xor-shift 17, `* B`; A and B fixed, the key drawn per seed (only its low 40 bits matter). They depend on the seed alone (draws 6-8 of the dataset stream).

**Design** (choices made):
- Keys are recovered at run time from samples: the first 8,192 link pairs (two linked IDs are usually one person's, numbers at most 8 apart) for the user key, then event pairs of one person in rows from the start of up to 96 chunks (numbers at most 64 apart) for the event key. Key bits come from the borrow pattern of `x1 - x2 = A*d` (`pair_bits`), grown from a seed pair and scored on up to 2,048 sampled IDs. Nothing about a dataset is hard-coded.
- An ID is decoded only if it is the prefix plus canonical lowercase base 36 (1-8 digits, no leading zero, below 2^40). That is exactly the set of strings that re-encoding the decoded number gives back, so a number stands for one string. Upper-case, padded, quoted-away or foreign IDs never decode.
- The run uses the general path from the start when a key is not recovered (score under 0.5; a wrong key scores ~0), or fewer than half the sampled event IDs or user IDs decode, or a guard fails (window over 2^32 ms, over 255 categories, over 32 threads, an ingest_ts that is not whole seconds near the window, which rules.md says never happens). `PIPELINE_FAST=0` forces it for A/B.
- Events: every copy becomes one 20-byte record (event number, ingest seconds, ts offset, user number or a reference to an undecoded user's key, price | category) in the shard of its event number (2^16 numbers per shard). Dedup is an array per shard (smallest ingest_ts wins; equal ones are identical copies by rules.md). Copies whose event_id does not decode are deduplicated by key.
- Survivors come out in number order, so a person's events are one run. A person (`person = number >> 9`) is finished in its shard only if all its surviving events form exactly one run there and it is simple: its linked aliases are all in one link set holding nothing but its own aliases (all below 64). Its events are grouped by person key (the set's root for a linked alias, else the user number), so unlinked aliases stay separate persons. Names: the root's key (the set's smallest ID, as the union-find already computes) or the re-encoded ID itself.
- Everyone else goes to the unchanged general `sessionize` under the name `links::Names` gives: undecoded users, people split over two shards or two runs (event revisions to another person do this), people with an odd event ID, shared devices, upper-cased link variants, chains with many aliases. Clicks follow the same rule through one prefetched lookup (`home`).
- Link classification runs on one extra thread while the others parse events (only clicks need it).

**Tests added:**
- Unit: scramble round trip, `encode` against the generator's own scramble/base36, the SWAR decoder against the scalar one on 300k random strings at every chunk position, key recovery on 20 synthetic keys.
- `tools/genkeys` (the generator's own `plan.rs`) prints each seed's true keys; `scripts/key_test.py` checks recovery on 22 seeds from 20k to 3M events: **22/22 exact** (both keys).
- Differential suite extended to 33 cases (25 seeds from 20k to 3M events, 2 long-ID, 6 adversarial from `scripts/make_adversarial.py`: every user ID remapped to non-generator strings, a tenth remapped, and added upper-case/space-padded variants with links back): **33/33 MATCH**. The `all` cases switch the fast path off (no user key); `tenth` and `variants` run both paths in one run (seed 2 variants: 97,720 events fast, 99,930 general).
- 20M and 60M against v1 (independent implementation): **exact match**; peak private 1.37 GiB (20M) and 3.43 GiB (60M), vs 2.5 / 7.6 GiB for v1. At 20M, 17.6M of 19.6M surviving events and 4.83M of 5.11M valid clicks took the fast path.
- Fixtures 36/36 (their IDs are not generator-format: general path), dev answer MATCH with the fast path on and off.

| # | Change | Result | Decision |
| --- | --- | --- | --- |
| F2 | Link classification on its own thread during event parsing; complex persons folded into `home` so a click needs one lookup, prefetched 16 clicks ahead; ingest seconds by unsigned division (a7fbed1). A/B in the same job: 2^17 and 2^18 events per shard (`PIPELINE_SHARD_BITS`), PLAIN output | CPU-s v10 / F1 / F2 / s17 / s18 / plain: EPYC 7763 7.10 / 5.92 / **5.71** / 5.74 / 5.70 / 5.76; Xeon 6975P 5.72 / 4.88 / **4.69** / 4.66 / 4.60 / 4.69; EPYC 9V45 4.76 / 3.73 / **3.50** / 3.64 / 3.71 / 3.50. Container warm v10 / F1 / F2: 2.012 / 1.785 / **1.685**; 1.590 / 1.418 / **1.343**; 1.327 / 1.131 / **1.069** | F2 kept (-3 to -6% CPU over F1 on all three). Larger shards and PLAIN: no consistent gain, defaults unchanged (2^16, dictionary output). Tagged v11 |
| F3 | Integer person keys for everyone the fast path does not finish in one shard: a linked person's key is its link root, an unlinked user's its number; they go through `sessionize` tagged (low byte 0xFE) and `Strings` names them when rows are written, so no row needs a name lookup. Only unlinked IDs that do not decode keep a string key (their own name). Plus link files cut into ~16 chunks per thread (teammate pipeline's latest change) | 20M: 1,947,071 of the 1,947,175 general-path events now carry integer keys (104 undecoded). b920a61 vs v11, same job, CPU-s: 5.73 -> 5.65, 5.63 -> 5.56, 4.82 -> 4.73 (-1.2 to -1.9%); ID linking 0.107 -> 0.099 s; warm wall -0.3 to -1.4% | Kept (small but on all three runners; exact on all gates) |
| F1 | Fast path + direct product/campaign lookups (c8e8a63) vs v10, same job | CPU-s (native, 4 threads) v10 / fast / general-only: EPYC 9V74 5.71 / **4.76** / 5.55; EPYC 7763 6.72 / **5.69** / 6.56; EPYC 9V74 5.68 / **4.72** / 5.52 (-15 to -17%). Container warm 1.624 -> 1.413, 1.915 -> 1.701, 1.626 -> 1.413 (-11 to -13%); cold -2 to -10%. Max RSS 2050 -> 1678 MiB | Kept. The general-only run (`PIPELINE_FAST=0`) is faster than v10 everywhere, so the fallback costs nothing |

### What's next (ranked by the v9 breakdown)
1. Events read+parse (~48% of warm wall): the partition scatter in `flush` (35% of that symbol is the two 32-byte record writes to 256 x 2 streams) -> software write-combining or fewer streams; the timestamp parser (~13% of the events callback); the tokenizer's per-field unquote (`emit`, ~27% of the tokenizer).
2. Sessionize (~20%): DenseIds probes and the counting-sort scatter in `group-events`; attribution's per-person sort and binary searches.
3. Output (exit 0.12 s on slow disks for 20 MB): fewer bytes still (revenue/events columns), or start writeback early (`sync_file_range`) so it overlaps the rest.
4. Dedup and ID linking (~9% together): hash-probe cache misses.

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
