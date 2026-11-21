#!/usr/bin/env python3
"""
Reads an output folder with pyarrow and DuckDB (independent of the polars-based
checker) and compares both against compare.py's loader. Usage: check_parquet.py OUT
"""
from pathlib import Path
import sys

import duckdb
import pyarrow.parquet as pq

from run_fixtures import SCHEMAS, load_actual

EXPECTED_ARROW = {
    "user_daily": {"user_id": "string", "day": "date32[day]", "sessions": "int64", "events": "int64",
                   "revenue_cents": "int64", "top_category": "string"},
    "campaign_daily": {"campaign_id": "string", "channel": "string", "day": "date32[day]", "sessions": "int64",
                       "purchasing_sessions": "int64", "revenue_cents": "int64", "assisted_sessions": "int64"},
}


def main():
    out = Path(sys.argv[1])
    ok = True
    for table, schema in SCHEMAS.items():
        files = sorted((out / table).glob("*.parquet"))
        reference = sorted(load_actual(out / table, schema).rows(), key=repr)
        arrow_rows = []
        for path in files:
            meta = pq.ParquetFile(path)
            types = {f.name: str(f.type) for f in meta.schema_arrow}
            if types != EXPECTED_ARROW[table]:
                ok = False
                print(f"{path.name}: arrow schema {types}")
            logical = {meta.schema.column(i).name: str(meta.schema.column(i).logical_type)
                       for i in range(len(meta.schema))}
            if logical["day"] != "Date":
                ok = False
                print(f"{path.name}: day logical type {logical['day']}")
            arrow_rows += [tuple(r.values()) for r in pq.read_table(path).to_pylist()]
        arrow_rows.sort(key=repr)
        con = duckdb.connect()
        cols = ", ".join(schema)
        duck_rows = con.execute(
            f"SELECT {cols} FROM read_parquet('{(out / table).as_posix()}/*.parquet')").fetchall()
        duck_rows.sort(key=repr)
        print(f"{table}: {len(files)} files, polars {len(reference)} rows, pyarrow {len(arrow_rows)}, "
              f"duckdb {len(duck_rows)}; pyarrow==polars {arrow_rows == reference}, duckdb==polars {duck_rows == reference}")
        ok &= arrow_rows == reference and duck_rows == reference
    print("PARQUET OK" if ok else "PARQUET MISMATCH")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
