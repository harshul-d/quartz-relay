#!/usr/bin/env python3
"""Filters `perf annotate --stdio -l --full-paths` output down to the hottest
source lines of each symbol and appends each line's source text.

Usage: perf annotate ... | srclines.py SYSROOT [MIN_PERCENT]
Paths under /rustc/<hash>/ are mapped into SYSROOT's rust-src component."""
import re
import sys

sysroot = sys.argv[1]
minimum = float(sys.argv[2]) if len(sys.argv) > 2 else 0.4
cache = {}


def text(path, line):
    path = re.sub(r"^/rustc/[0-9a-f]+/", sysroot + "/lib/rustlib/src/rust/", path)
    if path not in cache:
        try:
            with open(path, encoding="utf-8", errors="replace") as f:
                cache[path] = f.read().splitlines()
        except OSError:
            cache[path] = []
    lines = cache[path]
    return lines[line - 1].strip()[:110] if 0 < line <= len(lines) else ""


shown = 0
for raw in sys.stdin:
    if "Percent |" in raw:
        # perf prints a symbol's line summary before its disassembly header.
        samples = re.search(r"\((\d+) samples", raw)
        print(f"--- (lines above: one symbol, {samples.group(1) if samples else '?'} samples)")
        shown = 0
        continue
    m = re.match(r"^\s+([0-9]+\.[0-9]+)\s+(\S+):([0-9]+)\s*$", raw)
    if m and float(m.group(1)) >= minimum and shown < 25:
        path, line = m.group(2), int(m.group(3))
        short = "/".join(path.split("/")[-2:])
        print(f"{float(m.group(1)):6.2f}  {short}:{line}  {text(path, line)}")
        shown += 1
