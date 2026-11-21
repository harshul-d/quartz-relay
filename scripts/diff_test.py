#!/usr/bin/env python3
"""
Differential test: generates datasets with many seeds, runs the reference
(reference/reference.py) and the pipeline binary on each, and requires an
exact match from scripts/compare.py on every one.

Usage:
  python scripts/diff_test.py --generator GEN --binary BIN --data DIR [--quick]
  python scripts/diff_test.py --generator GEN --find-leap   # seeds whose window holds 2028-02-29

Datasets and reference outputs are cached under DIR (keep it outside the
repo); the binary's output is recomputed every run.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

HERE = Path(__file__).resolve().parent
REPO = HERE.parent

# (seed, events). Clicks are events / 4. The windows of seeds 15
# (2028-02-17 to 03-02) and 249 (2028-02-16 to 03-01, so Feb 29 is its last
# day) hold 2028-02-29 (found with --find-leap).
CASES = [
    (2, 200_000), (3, 300_000), (4, 400_000), (5, 500_000), (6, 600_000),
    (7, 750_000), (8, 1_000_000), (9, 1_250_000), (10, 1_500_000),
    (11, 2_000_000), (12, 250_000), (15, 1_000_000), (249, 400_000),
    (13, 20_000), (14, 35_000), (16, 60_000), (17, 100_000), (18, 150_000),
    (19, 350_000), (20, 450_000), (21, 900_000), (22, 1_750_000), (23, 2_250_000),
    (24, 2_600_000), (25, 3_000_000),
]
QUICK = [(2, 200_000), (249, 400_000), (15, 1_000_000)]
# Variants with a third of all IDs lengthened past 15 bytes (scripts/make_long_ids.py),
# for the pipeline's interned long-ID path.
LONG = [(2, 200_000), (12, 250_000)]
# User IDs rewritten by scripts/make_adversarial.py so they leave the generator's
# canonical form (all, a tenth) or gain upper-case and space-padded variants.
# Full mode only: (mode, seed, events).
ADVERSARIAL = [(mode, seed, events) for mode in ("all", "tenth", "variants")
               for seed, events in ((2, 200_000), (12, 250_000))]


def generate(generator, seed, events, folder):
    if (folder / "manifest.json").is_file():
        return
    tmp = folder.with_name(folder.name + ".tmp")
    shutil.rmtree(tmp, ignore_errors=True)
    subprocess.run([generator, "--seed", str(seed), "--events", str(events),
                    "--clicks", str(events // 4), "--output", str(tmp)], check=True)
    tmp.rename(folder)


def run(cmd, data, out):
    shutil.rmtree(out, ignore_errors=True)
    out.mkdir(parents=True)
    env = dict(os.environ, DATA_DIR=str(data), OUTPUT_DIR=str(out))
    start = time.perf_counter()
    subprocess.run(cmd, env=env, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return time.perf_counter() - start


def find_leap(generator, limit):
    leap = datetime.datetime(2028, 2, 29, tzinfo=datetime.timezone.utc)
    with tempfile.TemporaryDirectory() as tmp:
        for seed in range(1, limit):
            out = Path(tmp) / str(seed)
            subprocess.run([generator, "--seed", str(seed), "--events", "1000", "--clicks", "250",
                            "--output", str(out)], check=True, stdout=subprocess.DEVNULL)
            manifest = json.loads((out / "manifest.json").read_text())
            shutil.rmtree(out)
            start, end = (datetime.datetime.fromisoformat(manifest[k].replace("Z", "+00:00"))
                          for k in ("window_start", "window_end"))
            if start <= leap < end:
                print(f"seed {seed}: {manifest['window_start']} to {manifest['window_end']}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--generator", required=True)
    parser.add_argument("--binary")
    parser.add_argument("--data")
    parser.add_argument("--quick", action="store_true")
    parser.add_argument("--find-leap", action="store_true")
    args = parser.parse_args()
    if args.find_leap:
        find_leap(args.generator, 1000)
        return 0

    data_root = Path(args.data).resolve()
    data_root.mkdir(parents=True, exist_ok=True)
    failures = []
    cases = [(seed, events, None) for seed, events in (QUICK if args.quick else CASES)]
    cases += [(seed, events, "long") for seed, events in LONG]
    if not args.quick:
        cases += [(seed, events, mode) for mode, seed, events in ADVERSARIAL]
    for seed, events, kind in cases:
        name = f"diff-s{seed}-e{events}"
        data = data_root / name
        generate(args.generator, seed, events, data)
        if kind == "long":
            name = f"long-s{seed}-e{events}"
            source, data = data, data_root / name
            if not (data / "manifest.json").is_file():
                subprocess.run([sys.executable, str(HERE / "make_long_ids.py"), str(source), str(data)], check=True)
        elif kind:
            name = f"adv-{kind}-s{seed}-e{events}"
            source, data = data, data_root / name
            if not (data / "manifest.json").is_file():
                tmp = data.with_name(name + ".tmp")
                subprocess.run([sys.executable, str(HERE / "make_adversarial.py"), str(source), str(tmp),
                                "--mode", kind, "--seed", str(seed)], check=True, stdout=subprocess.DEVNULL)
                shutil.rmtree(data, ignore_errors=True)
                tmp.rename(data)
        ref_out = data_root / (name + "-ref")
        if not (ref_out / "done").is_file():
            seconds = run([sys.executable, str(REPO / "reference" / "reference.py")], data, ref_out)
            (ref_out / "done").write_text(f"{seconds:.1f}\n")
        bin_out = data_root / (name + "-bin")
        seconds = run([args.binary], data, bin_out)
        result = subprocess.run([sys.executable, str(HERE / "compare.py"), str(bin_out), str(ref_out)],
                                capture_output=True, text=True)
        verdict = result.stdout.strip().splitlines()[-1] if result.stdout.strip() else "ERROR"
        manifest = json.loads((data / "manifest.json").read_text())
        print(f"{name:24} {manifest['window_start'][:10]}  binary {seconds:6.2f}s  {verdict}", flush=True)
        if result.returncode != 0:
            failures.append(name)
            print(result.stdout, result.stderr)
    print(f"\n{len(failures)} failures" if failures else "\nALL MATCH")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
