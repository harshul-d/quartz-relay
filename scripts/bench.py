#!/usr/bin/env python3
"""
Times the pipeline binary on one dataset and reports peak memory.

Usage: python scripts/bench.py BINARY DATA_DIR OUT_DIR [RUNS]

Peak memory is sampled with psutil every 10 ms: the working set / RSS
(includes touched pages of the memory-mapped input) and, where the OS
reports it, private committed memory (the heap).
"""
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time

import psutil


def one_run(binary, data, out):
    shutil.rmtree(out, ignore_errors=True)
    Path(out).mkdir(parents=True)
    env = dict(os.environ, DATA_DIR=str(data), OUTPUT_DIR=str(out))
    start = time.perf_counter()
    proc = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    ps = psutil.Process(proc.pid)
    peak_rss = peak_private = 0
    while proc.poll() is None:
        try:
            info = ps.memory_info()
            peak_rss = max(peak_rss, getattr(info, "peak_wset", info.rss))
            peak_private = max(peak_private, getattr(info, "peak_pagefile", 0))
        except psutil.Error:
            pass
        time.sleep(0.01)
    seconds = time.perf_counter() - start
    stderr = proc.stderr.read().decode(errors="replace")
    if proc.returncode != 0:
        sys.exit(f"exit {proc.returncode}\n{stderr}")
    return seconds, peak_rss, peak_private, stderr


def main():
    binary, data, out = sys.argv[1:4]
    runs = int(sys.argv[4]) if len(sys.argv) > 4 else 3
    times = []
    for run in range(runs):
        seconds, rss, private, stderr = one_run(binary, data, out)
        times.append(seconds)
        print(f"run {run + 1}: {seconds:.3f}s  peak working set {rss / 2**30:.2f} GiB  "
              f"peak private {private / 2**30:.2f} GiB")
    print(stderr.rstrip())
    print(f"median {statistics.median(times):.3f}s  min {min(times):.3f}s")


if __name__ == "__main__":
    main()
