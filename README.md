# Shopping Sessions (Rust)

A solo entry for the Databricks "Shopping Sessions" track of the Builders Buildathon.
The pipeline reads the raw clickstream in `$DATA_DIR` (events, clicks, ID links,
products, campaigns and a manifest) and writes two Parquet tables,
`$OUTPUT_DIR/user_daily/` and `$OUTPUT_DIR/campaign_daily/`, exactly as the track's
`rules.md` defines: CSV dialect quirks, linked IDs, event dedup, validation, late
events, 30-minute sessions, last-click attribution and 7-day assists.

It is one native Rust binary that uses every core:

- Input files are memory-mapped and cut into newline-aligned chunks shared by all threads.
- A hand-written byte parser implements the dialect (quotes only around whole fields,
  `""` is null, `\r` is data, spaces are significant, malformed lines are dropped).
- Every ID of up to 15 bytes is packed into an exact 128-bit key whose numeric order
  is the byte-wise string order; longer IDs are interned. No hash is ever used as identity.
- Rows are hash-partitioned, deduplicated per partition, regrouped by person (union-find
  over the links) and sessionized per person in parallel.
- Timestamps are integer milliseconds with exact calendar validation.

## Layout

| Path | What |
| --- | --- |
| `src/` | The pipeline (`main.rs` orchestration, `csv.rs` dialect, `ts.rs` timestamps, `key.rs` ID keys, `out.rs` Parquet, `par.rs` threads) |
| `Dockerfile` | Multi-stage build: official Rust image, then a minimal Debian image running the static binary as uid 1000 |
| `reference/reference.py` | A slow, plain-Python reference implementation used as the test oracle |
| `fixtures/` | The 36 hand-written edge cases from the starter, with expected outputs |
| `scripts/run_fixtures.py`, `scripts/compare.py`, `scripts/docker_run.sh` | Test scripts from the starter |
| `scripts/diff_test.py` | Differential test: many generated datasets, reference vs binary, exact match required |
| `PROGRESS.md` | Status, decisions, test results and timings |

## Build

```sh
cargo build --release                      # local build
docker buildx build --platform linux/amd64 --load -t shopping-sessions:latest .
```

## Run

```sh
DATA_DIR=/path/to/data OUTPUT_DIR=/path/to/out target/release/shopping-sessions
scripts/docker_run.sh /path/to/data /path/to/out    # the image, with the scorer's flags
```

`NCPU` sets the thread count (default: all cores). Progress goes to stderr.

## Test

Needs Python 3.10+ with `polars` and `pyarrow`.

```sh
python scripts/run_fixtures.py fixtures -- target/release/shopping-sessions      # 36 fixtures
python scripts/run_fixtures.py fixtures -- python reference/reference.py        # same, for the reference
python scripts/compare.py OUT dev-answer                                        # dev set vs published answer
python scripts/diff_test.py --generator generate --binary target/release/shopping-sessions --data ../data/diff
```

## Open-source libraries

Rust (see `Cargo.lock` for the exact versions and transitive dependencies):

- [memchr](https://github.com/BurntSushi/memchr): SIMD byte search (MIT / Unlicense)
- [memmap2](https://github.com/RazrFalcon/memmap2-rs): memory-mapped files (MIT / Apache-2.0)
- [parquet](https://github.com/apache/arrow-rs): Apache Parquet writer from the Apache Arrow Rust project (Apache-2.0)

Python, for the reference and the tests only:

- [pyarrow](https://arrow.apache.org/docs/python/): writes the reference's Parquet (Apache-2.0)
- [polars](https://pola.rs/): reads Parquet in the starter's test scripts (MIT)

## License

MIT, see `LICENSE`.
