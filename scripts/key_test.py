#!/usr/bin/env python3
"""
Key-recovery test: for many seeds and sizes, runs the pipeline binary on a
generated dataset and checks the keys its fast path recovers against the keys
the generator really drew (tools/genkeys, which compiles the generator's own
plan code).

The pipeline prints exactly one stderr line, either
    fast-path keys user=0x<hex> event=0x<hex>
or
    fast-path off: <reason>

Usage:
  python scripts/key_test.py [--generator GEN] [--genkeys GK] [--binary BIN] [--data DIR]
  python scripts/key_test.py --generate-only [--jobs 2]   # just fill the dataset cache

Datasets are cached under DIR (default ../data/keys, outside the repo); each is
generated once into a .tmp folder and renamed when complete. Exits 1 on any
MISMATCH, OFF, MISSING (no fast-path line) or ERROR (binary failed).
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
EXE = ".exe" if os.name == "nt" else ""

# (seed, events). Clicks are events / 4. Seeds 15 and 249 have windows that
# hold 2028-02-29; 0 and 2^64-1 are the extreme seeds. About 14.8M events in
# all, roughly 1.4 GB on disk.
CASES = [
    (1, 20_000), (0, 25_000), (2, 30_000), (18446744073709551615, 35_000),
    (3, 50_000), (8, 60_000), (7, 75_000), (42, 100_000), (99, 150_000),
    (123, 200_000), (500, 250_000), (777, 300_000), (249, 400_000),
    (1000, 500_000), (4242, 600_000), (31337, 750_000), (15, 1_000_000),
    (65535, 1_250_000), (100003, 1_500_000), (2147483647, 2_000_000),
    (4294967303, 2_500_000), (9223372036854775813, 3_000_000),
]

KEYS = re.compile(r"fast-path keys user=0x([0-9a-fA-F]+) event=0x([0-9a-fA-F]+)")
OFF = re.compile(r"fast-path off:\s*(.*)")
MASK = (1 << 40) - 1


def generate(generator, seed, events, folder, threads):
    if (folder / "manifest.json").is_file():
        return False
    tmp = folder.with_name(folder.name + ".tmp")
    shutil.rmtree(tmp, ignore_errors=True)
    subprocess.run([generator, "--seed", str(seed), "--events", str(events),
                    "--clicks", str(events // 4), "--output", str(tmp),
                    "--threads", str(threads)],
                   check=True, stdout=subprocess.DEVNULL)
    # On Windows a scanner or indexer can briefly hold a just-written file,
    # which makes the rename fail with "Access is denied"; retry a while.
    for attempt in range(20):
        try:
            tmp.rename(folder)
            return True
        except PermissionError:
            if attempt == 19:
                raise
            time.sleep(0.5 * (attempt + 1))


def true_keys(genkeys, seed):
    out = subprocess.run([genkeys, "--seed", str(seed)], check=True,
                         capture_output=True, text=True).stdout
    keys = dict(part.split("=", 1) for part in out.split())
    return {name: int(value, 16) & MASK for name, value in keys.items()}


def run_pipeline(binary, data, threads):
    out = Path(tempfile.mkdtemp(prefix="keys-out-"))
    env = dict(os.environ, DATA_DIR=str(data), OUTPUT_DIR=str(out), NCPU=str(threads))
    try:
        start = time.perf_counter()
        result = subprocess.run([binary], env=env, capture_output=True, text=True,
                                errors="replace")
        return result, time.perf_counter() - start
    finally:
        shutil.rmtree(out, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--generator", default=str(REPO / "tools" / "gen" / "target" / "release" / f"generate{EXE}"))
    parser.add_argument("--genkeys", default=str(REPO / "tools" / "genkeys" / "target" / "release" / f"genkeys{EXE}"))
    parser.add_argument("--binary", default=str(REPO / "target" / "release" / f"pipeline{EXE}"))
    parser.add_argument("--data", default=str(REPO.parent / "data" / "keys"))
    parser.add_argument("--threads", type=int, default=4,
                        help="threads per generator / pipeline run (NCPU for the pipeline)")
    parser.add_argument("--jobs", type=int, default=1,
                        help="datasets generated at once (keep jobs * threads <= 4)")
    parser.add_argument("--generate-only", action="store_true")
    args = parser.parse_args()

    data_root = Path(args.data).resolve()
    data_root.mkdir(parents=True, exist_ok=True)
    folder = lambda seed, events: data_root / f"s{seed}-e{events}"

    # Largest first so the parallel jobs finish together.
    pending = sorted((c for c in CASES if not (folder(*c) / "manifest.json").is_file()),
                     key=lambda c: -c[1])
    if pending:
        print(f"generating {len(pending)} datasets into {data_root}", flush=True)

        def make(case):
            start = time.perf_counter()
            generate(args.generator, case[0], case[1], folder(*case), args.threads)
            print(f"  generated s{case[0]}-e{case[1]} in {time.perf_counter() - start:.1f}s", flush=True)

        with ThreadPoolExecutor(max(1, args.jobs)) as pool:
            list(pool.map(make, pending))
    if args.generate_only:
        return 0

    failures = []
    print(f"{'seed':>20} {'events':>9}  {'true user':>12} {'true event':>12}  "
          f"{'got user':>12} {'got event':>12}  {'time':>6}  verdict", flush=True)
    for seed, events in CASES:
        want = true_keys(args.genkeys, seed)
        result, seconds = run_pipeline(args.binary, folder(seed, events), args.threads)
        text = result.stderr + "\n" + result.stdout
        keys, off = KEYS.search(text), OFF.search(text)
        got_user = got_event = "-"
        if keys:
            got = (int(keys.group(1), 16), int(keys.group(2), 16))
            got_user, got_event = (f"0x{v:x}" for v in got)
            verdict = "MATCH" if got == (want["user"], want["event"]) else "MISMATCH"
        elif off:
            verdict = f"OFF ({off.group(1).strip()})"
        else:
            verdict = "MISSING (no fast-path line)"
        if result.returncode != 0:
            verdict += f" ERROR (exit {result.returncode})"
        print(f"{seed:>20} {events:>9}  {'0x%x' % want['user']:>12} {'0x%x' % want['event']:>12}  "
              f"{got_user:>12} {got_event:>12}  {seconds:5.2f}s  {verdict}", flush=True)
        if verdict != "MATCH":
            failures.append((seed, events, verdict))
            if result.returncode != 0:
                print(result.stderr[-2000:], file=sys.stderr)

    counts = {}
    for *_, verdict in failures:
        word = verdict.split()[0]
        counts[word] = counts.get(word, 0) + 1
    print(f"\n{len(CASES) - len(failures)}/{len(CASES)} MATCH"
          + "".join(f", {n} {word}" for word, n in sorted(counts.items())))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
