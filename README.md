# Click-stream sessionization pipeline (Rust)

The pipeline reads the raw clickstream in `$DATA_DIR` (events, clicks, ID links,
products, campaigns and a manifest) and writes two Parquet tables,
`$OUTPUT_DIR/user_daily/` and `$OUTPUT_DIR/campaign_daily/`, exactly as the
specification (`rules.md`) defines: CSV dialect quirks, linked IDs, event dedup, validation, late
events, 30-minute sessions, last-click attribution and 7-day assists.

It is one native Rust binary that uses every core:

- Input files are cut into newline-aligned chunks shared by all threads and read with positional reads into reused per-thread buffers.
- A hand-written AVX2 tokenizer (bit masks of separators and quotes per 64-byte block) implements the dialect (quotes only around whole fields,
  `""` is null, `\r` is data, spaces are significant, malformed lines are dropped).
- Every ID of up to 15 bytes is packed into an exact 128-bit key whose numeric order
  is the byte-wise string order; longer IDs are interned. No hash is ever used as identity.
- Rows are hash-partitioned, deduplicated per partition, regrouped by person (union-find
  over the links) and sessionized per person in parallel.
- Timestamps are integer milliseconds with exact calendar validation.

## Layout

| Path | What |
| --- | --- |
| `src/` | The pipeline (`main.rs` orchestration, `csv.rs` dialect and input, `links.rs` linked IDs, `ts.rs` timestamps, `key.rs` ID keys, `out.rs` Parquet writer, `par.rs` threads) |
| `Dockerfile` | Multi-stage build: official Rust image, then a minimal Debian image running the static binary as uid 1000 |
| `reference/reference.py` | A slow, plain-Python reference implementation used as the test oracle |
| `fixtures/` | The 36 hand-written edge cases from the starter, with expected outputs |
| `scripts/run_fixtures.py`, `scripts/compare.py`, `scripts/docker_run.sh` | Test scripts from the starter |
| `scripts/diff_test.py` | Differential test: many generated datasets, reference vs binary, exact match required |
| `scripts/check_parquet.py` | Reads an output folder with pyarrow and DuckDB and checks schema, logical types and rows |
| `scripts/bench.py`, `scripts/verify.ps1` | Timing with peak memory; the full local check (tests, fixtures, dev, differential) |
| `PROGRESS.md` | Status, decisions, test results and timings |

## Build

```sh
cargo build --release                      # local build
docker buildx build --platform linux/amd64 --load -t pipeline:latest .
```

## Run

```sh
DATA_DIR=/path/to/data OUTPUT_DIR=/path/to/out target/release/pipeline
scripts/docker_run.sh /path/to/data /path/to/out    # the image, with the scorer's flags
```

`NCPU` sets the thread count (default: all cores). Progress goes to stderr.

## Test

Needs Python 3.10+ with `polars` and `pyarrow`.

```sh
python scripts/run_fixtures.py fixtures -- target/release/pipeline      # 36 fixtures
python scripts/run_fixtures.py fixtures -- python reference/reference.py        # same, for the reference
python scripts/compare.py OUT dev-answer                                        # dev set vs published answer
python scripts/diff_test.py --generator generate --binary target/release/pipeline --data ../data/diff
```

## Open-source libraries

Rust (see `Cargo.lock` for the exact versions and transitive dependencies):

- [memchr](https://github.com/BurntSushi/memchr): SIMD byte search (MIT / Unlicense)
- [mimalloc](https://github.com/purpleprotocol/mimalloc_rust): Rust bindings to Microsoft's mimalloc allocator (MIT)

The Parquet writer (`src/out.rs`) is hand-written: PLAIN encoding, no compression, Thrift compact metadata.

Python, for the reference and the tests only:

- [pyarrow](https://arrow.apache.org/docs/python/): writes the reference's Parquet (Apache-2.0)
- [polars](https://pola.rs/): reads Parquet in the starter's test scripts (MIT)
- [duckdb](https://duckdb.org/): a second, independent Parquet reader in `scripts/check_parquet.py` (MIT)
- [psutil](https://github.com/giampaolo/psutil): peak memory in `scripts/bench.py` (BSD-3-Clause)

## License

MIT, see `LICENSE`.
