#!/usr/bin/env python3
"""
Copies a dataset, giving a deterministic third of all IDs (user, event, click
and link IDs) a suffix that makes them longer than 15 bytes, so the
pipeline's interned long-ID path meets real data, including long IDs that
share a prefix with short ones. Quoting and malformed lines are preserved.

Usage: make_long_ids.py SRC DST
"""
import hashlib
from pathlib import Path
import shutil
import sys

SUFFIX = "-with-a-long-tail"
FIELDS = {"events": (0, 1), "clicks": (0, 1), "links": (0, 1)}


def lengthen(field: str) -> str:
    quoted = len(field) >= 2 and field[0] == '"' and field[-1] == '"'
    value = field[1:-1] if quoted else field
    if not value or '"' in value:
        return field
    if hashlib.blake2b(value.encode(), digest_size=2).digest()[0] % 3 != 0:
        return field
    value += SUFFIX
    return f'"{value}"' if quoted else value


def main():
    src, dst = map(Path, sys.argv[1:])
    if dst.exists():
        shutil.rmtree(dst)
    shutil.copytree(src, dst)
    for folder, positions in FIELDS.items():
        widths = {"events": 7, "clicks": 4, "links": 2}[folder]
        for path in sorted((dst / folder).glob("*.csv")):
            lines = path.read_bytes().split(b"\n")
            out = [lines[0]]
            for line in lines[1:]:
                fields = line.decode().split(",")
                if len(fields) == widths:
                    for at in positions:
                        fields[at] = lengthen(fields[at])
                out.append(",".join(fields).encode())
            path.write_bytes(b"\n".join(out))


if __name__ == "__main__":
    main()
