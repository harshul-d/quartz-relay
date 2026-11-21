#!/usr/bin/env python3
"""Times pipeline images on a Linux CI runner the way the scorer does and
writes a Markdown report.

Usage: bench.py --data DIR --variants FILE --work DIR --report FILE [--reps N]

Each variant line in FILE is `name git-ref [KEY=VALUE ...]`; the image is
built from that ref (HEAD = the checkout) and the KEY=VALUE pairs are passed
to the container as extra environment (for flag-gated experiments).

Measured per variant, alternating variants within every repetition so they
share one machine state:
  * container wall time (docker run start to exit, the flags of
    scripts/docker_run.sh), warm page cache and cold (caches dropped);
  * the same image with /bin/true as the command: pure container overhead;
  * the binary run natively under /usr/bin/time -v (memory, faults, switches);
  * the binary's own phase log on stderr.
Also a raw read floor: `cat` of every input file with one reader per CPU.
"""
import argparse
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

# Flags a target-cpu=x86-64-v3 build needs (as named in /proc/cpuinfo).
V3_FLAGS = ["avx", "avx2", "bmi1", "bmi2", "f16c", "fma", "abm", "movbe", "xsave"]
V4_FLAGS = ["avx512f", "avx512bw", "avx512cd", "avx512dq", "avx512vl"]
ROOT = Path(__file__).resolve().parent.parent

PHASES = ["lookups", "links", "events parsed", "events deduplicated", "clicks parsed", "sessionized", "written",
          "end of main"]
PHASE_LABELS = {"lookups": "lookups", "links": "ID linking", "events parsed": "events read+parse",
                "events deduplicated": "dedup", "clicks parsed": "clicks read+parse",
                "sessionized": "sessionize+attribution", "written": "output writing",
                "end of main": "report+drops"}


def log(*args):
    print(*args, file=sys.stderr, flush=True)


def run(cmd, **kwargs):
    log("+", " ".join(map(str, cmd)))
    return subprocess.run(cmd, check=True, **kwargs)


def cpu_info():
    flags = set()
    model = "?"
    with open("/proc/cpuinfo") as f:
        for line in f:
            if line.startswith("flags") and not flags:
                flags = set(line.split(":", 1)[1].split())
            elif line.startswith("model name") and model == "?":
                model = line.split(":", 1)[1].strip()
    lscpu = subprocess.run(["lscpu"], capture_output=True, text=True).stdout
    return model, os.cpu_count(), flags, lscpu


THP_PATH = "/sys/kernel/mm/transparent_hugepage/enabled"


def thp_setting(path=THP_PATH):
    try:
        return re.search(r"\[(\w+)\]", Path(path).read_text()).group(1)
    except (OSError, AttributeError):
        return "?"


def set_thp(mode):
    if mode and thp_setting() != mode:
        subprocess.run(["sudo", "sh", "-c", f"echo {mode} > {THP_PATH}"], check=True)


def warm_cache(data):
    subprocess.run(["sh", "-c", f"find '{data}' -type f -print0 | xargs -0 cat > /dev/null"], check=True)


def drop_caches():
    subprocess.run(["sync"], check=True)
    subprocess.run(["sudo", "sh", "-c", "echo 3 > /proc/sys/vm/drop_caches"], check=True)


def remove(path):
    # Files the container wrote belong to uid 1000, not to the runner user.
    subprocess.run(["sudo", "rm", "-rf", str(path)], check=True)


def docker_args(image, data, out, scratch, cpus, mem, env=(), entry=None):
    """The docker run command of scripts/docker_run.sh (keep in sync)."""
    args = ["docker", "run", "--rm", "--platform", "linux/amd64",
            "--network", "none", "--read-only", "--tmpfs", "/tmp:rw,size=64m,mode=1777",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            "--pids-limit", "4096", "--cpus", str(cpus), "--memory", mem, "--memory-swap", mem,
            "--user", "1000:1000",
            "-v", f"{data}:/data:ro,z", "-v", f"{out}:/output:z", "-v", f"{scratch}:/scratch:z",
            "-e", "DATA_DIR=/data", "-e", "OUTPUT_DIR=/output", "-e", "SCRATCH_DIR=/scratch", "-e", "HOME=/scratch",
            "-e", f"NCPU={int(float(cpus))}", "-e", f"MEM_GB={mem.rstrip('g')}"]
    if any(item.startswith("NCPU=") for item in env):  # a thread-count experiment replaces NCPU
        at = args.index(f"NCPU={int(float(cpus))}")
        del args[at - 1:at + 1]
    for item in env:
        args += ["-e", item]
    if entry:
        args += ["--entrypoint", entry]
    args.append(image)
    return args


def parse_stderr(text):
    phases, counters = {}, {}
    for line in text.splitlines():
        m = re.match(r"^\s*([0-9.]+)s (.+?)\s*$", line)
        if m:
            phases[m.group(2)] = float(m.group(1))
            continue
        m = re.match(r"^cpu (\S+) ([0-9.]+)$", line)
        if m:
            counters[m.group(1)] = float(m.group(2))
        m = re.match(r"^epoch (\S+) ([0-9]+)$", line)
        if m:
            phases["epoch " + m.group(1)] = int(m.group(2))
    return phases, counters


def phase_durations(phases, wall):
    """Cumulative log times -> per-phase seconds, plus the time outside main."""
    out, previous = {}, 0.0
    for name in PHASES:
        if name in phases:
            out[PHASE_LABELS[name]] = phases[name] - previous
            previous = phases[name]
    if "end of main" in phases or "written" in phases:
        out["outside main (start, exit, container)"] = wall - previous
    e = {k[6:]: v for k, v in phases.items() if k.startswith("epoch ")}
    if all(k in e for k in ("run-start", "main-start", "main-end", "run-end")):
        out["start: run start -> main"] = (e["main-start"] - e["run-start"]) / 1e9
        out["exit: end of main -> run end"] = (e["run-end"] - e["main-end"]) / 1e9
    return out


class Variant:
    def __init__(self, name, ref, env):
        # THP=always|madvise|never is a machine setting applied around this
        # variant's runs, not container environment.
        self.thp = next((e.split("=", 1)[1] for e in env if e.startswith("THP=")), None)
        env = [e for e in env if not e.startswith("THP=")]
        self.name, self.ref, self.env = name, ref, env
        self.image = f"pipeline:{name}"
        self.binary = None


def read_variants(path):
    variants = []
    for line in Path(path).read_text().splitlines():
        line = line.split("#", 1)[0].split()
        if line:
            variants.append(Variant(line[0], line[1], line[2:]))
    return variants


def build(variants, work, fallback_cpu):
    """Builds every variant's image from its ref; extracts its binary."""
    notes = []
    for v in variants:
        if v.ref == "HEAD":
            src = ROOT
        else:
            src = work / f"src-{v.name}"
            if not src.exists():
                run(["git", "-C", str(ROOT), "worktree", "add", "--detach", str(src), v.ref])
        dockerfile = src / "Dockerfile"
        build_args = ["docker", "build", "-q", "-t", v.image]
        if fallback_cpu:
            text = dockerfile.read_text()
            patched = text.replace("target-cpu=x86-64-v4", f"target-cpu={fallback_cpu}").replace(
                "target-cpu=x86-64-v3", f"target-cpu={fallback_cpu}")
            ci_file = work / f"Dockerfile.{v.name}"
            ci_file.write_text(patched)
            build_args += ["-f", str(ci_file)]
            notes.append(f"{v.name}: built for {fallback_cpu} (runner lacks the release target's instructions)")
        run(build_args + [str(src)], stdout=subprocess.DEVNULL)
        container = subprocess.run(["docker", "create", v.image], check=True, capture_output=True,
                                   text=True).stdout.strip()
        v.binary = work / f"bin-{v.name}"
        run(["docker", "cp", f"{container}:/usr/local/bin/pipeline", str(v.binary)])
        run(["docker", "rm", container], stdout=subprocess.DEVNULL)
    return notes


def timed_container(v, data, work, cpus, mem, entry=None, keep=None):
    out = work / "out"
    scratch = work / "scratch"
    for d in (out, scratch):
        if d.exists():
            remove(d)
        d.mkdir()
        d.chmod(0o777)
    args = docker_args(v.image, data, out, scratch, cpus, mem, v.env, entry)
    start_ns = time.time_ns()
    begin = time.perf_counter()
    p = subprocess.run(args, capture_output=True, text=True)
    wall = time.perf_counter() - begin
    p.stderr += f"\nepoch run-start {start_ns}\nepoch run-end {time.time_ns()}\n"
    if p.returncode != 0:
        raise SystemExit(f"{v.name}: exit {p.returncode}\n{p.stderr[-3000:]}")
    if keep is not None and not keep.exists():
        shutil.copytree(out, keep)
    return wall, p.stderr


TIME_FIELDS = {
    "Maximum resident set size (kbytes)": "max RSS MiB",
    "Minor (reclaiming a frame) page faults": "minor faults",
    "Major (requiring I/O) page faults": "major faults",
    "Voluntary context switches": "voluntary switches",
    "Involuntary context switches": "involuntary switches",
    "User time (seconds)": "user s",
    "System time (seconds)": "system s",
}


def timed_native(v, data, work, cpus):
    out = work / "native-out"
    if out.exists():
        remove(out)
    out.mkdir()
    env = dict(os.environ, DATA_DIR=str(data), OUTPUT_DIR=str(out), NCPU=str(int(float(cpus))))
    for item in v.env:
        key, _, value = item.partition("=")
        env[key] = value
    start_ns = time.time_ns()
    begin = time.perf_counter()
    p = subprocess.run(["/usr/bin/time", "-v", str(v.binary)], capture_output=True, text=True, env=env)
    wall = time.perf_counter() - begin
    p.stderr += f"\nepoch run-start {start_ns}\nepoch run-end {time.time_ns()}\n"
    if p.returncode != 0:
        raise SystemExit(f"{v.name} native: exit {p.returncode}\n{p.stderr[-3000:]}")
    stats = {}
    for line in p.stderr.splitlines():
        key, _, value = line.strip().rpartition(": ")
        if key in TIME_FIELDS:
            number = float(value)
            stats[TIME_FIELDS[key]] = number / 1024 if key.startswith("Maximum") else number
    return wall, p.stderr, stats


def timed_cat(data, readers):
    files = [str(p) for p in sorted(Path(data).rglob("*")) if p.is_file()]
    begin = time.perf_counter()
    subprocess.run(["xargs", "-0", "-P", str(readers), "-n", "4", "cat"],
                   input="\0".join(files).encode(), stdout=subprocess.DEVNULL, check=True)
    return time.perf_counter() - begin


def summary(values):
    values = sorted(values)
    med = statistics.median(values)
    return med, values[0], values[-1]


def fmt(values):
    if not values:
        return "-"
    med, low, high = summary(values)
    return f"{med:.3f} ({low:.3f}-{high:.3f})"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--variants", required=True)
    ap.add_argument("--work", required=True)
    ap.add_argument("--report", required=True)
    ap.add_argument("--reps", type=int, default=5)
    ap.add_argument("--mem", default="24g")
    ap.add_argument("--compare", default=str(ROOT / "scripts" / "compare.py"))
    a = ap.parse_args()

    data = Path(a.data).resolve()
    work = Path(a.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    model, cpus, flags, lscpu = cpu_info()
    missing_v3 = [f for f in V3_FLAGS if f not in flags]
    fallback = "x86-64-v2" if missing_v3 else None
    variants = read_variants(a.variants)
    notes = build(variants, work, fallback)
    trivial = Variant("trivial", None, [])

    container = {}  # (variant, mode) -> walls
    stderr_runs = {}  # (variant, mode) -> [(wall, stderr)]
    native = {}  # variant -> [(wall, stats, stderr)]
    cats = {}
    default_thp = thp_setting()
    default_defrag = thp_setting(Path(THP_PATH).with_name("defrag"))
    readers = sorted({int(cpus), 16})

    # Untimed warm-up: images, binaries and input into the page cache.
    for v in variants:
        set_thp(v.thp or default_thp)
        timed_container(v, data, work, cpus, a.mem, keep=work / f"result-{v.name}")
    for rep in range(a.reps):
        order = variants[rep % len(variants):] + variants[:rep % len(variants)]
        for v in order:
            set_thp(v.thp or default_thp)
            for mode in ("cold", "warm"):
                if mode == "cold":
                    drop_caches()
                wall, err = timed_container(v, data, work, cpus, a.mem)
                container.setdefault((v.name, mode), []).append(wall)
                stderr_runs.setdefault((v.name, mode), []).append((wall, err))
                log(f"rep {rep} {v.name} {mode}: {wall:.3f}s")
            for mode in ("cold", "warm"):
                if mode == "cold":
                    drop_caches()
                trivial.image = v.image
                wall, _ = timed_container(trivial, data, work, cpus, a.mem, entry="/bin/true")
                container.setdefault((f"{v.name} /bin/true", mode), []).append(wall)
            warm_cache(data)  # the /bin/true cold run dropped the input from the cache
            wall, err, stats = timed_native(v, data, work, cpus)
            native.setdefault(v.name, []).append((wall, stats, err))
            log(f"rep {rep} {v.name} native: {wall:.3f}s")
        set_thp(default_thp)
        for n in readers:
            for mode in ("cold", "warm"):
                if mode == "cold":
                    drop_caches()
                cats.setdefault((n, mode), []).append(timed_cat(data, n))

    # Correctness of the timed runs: every variant's 20M output against the first.
    checks = []
    base = work / f"result-{variants[0].name}"
    for v in variants[1:]:
        p = subprocess.run([sys.executable, a.compare, str(base), str(work / f"result-{v.name}")],
                           capture_output=True, text=True)
        last = (p.stdout.strip().splitlines() or ["?"])[-1]
        checks.append(f"{v.name} vs {variants[0].name}: {last}")

    sha = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    data_bytes = sum(p.stat().st_size for p in data.rglob("*") if p.is_file())
    md = [f"# Benchmark {sha}", ""]
    md.append(f"- CPU: {model}; {cpus} logical CPUs; x86-64-v3 flags "
              f"{'all present' if not missing_v3 else 'MISSING ' + ' '.join(missing_v3)}; AVX-512: "
              f"{'yes' if all(f in flags for f in V4_FLAGS) else 'no'}")
    for note in notes:
        md.append(f"- NOTE: {note}")
    md.append(f"- Data: `{data.name}`, {data_bytes / 1e9:.2f} GB; {a.reps} repetitions; docker flags of "
              f"scripts/docker_run.sh with --cpus {cpus} --memory {a.mem}")
    md.append(f"- Transparent huge pages: runner default `{default_thp}`, defrag `{default_defrag}`")
    md.append("- Variants: " + "; ".join(
        f"`{v.name}` = {v.ref} {' '.join(v.env)}{' THP=' + v.thp if v.thp else ''}".strip() for v in variants))
    md += ["", "## Wall time, seconds: median (min-max)", "", "| Run | cold | warm |", "| --- | --- | --- |"]
    names = [v.name for v in variants] + [f"{v.name} /bin/true" for v in variants]
    for name in names:
        md.append(f"| {name} | {fmt(container.get((name, 'cold'), []))} | {fmt(container.get((name, 'warm'), []))} |")
    for v in variants:
        md.append(f"| {v.name} native (warm) | - | {fmt([w for w, _, _ in native[v.name]])} |")
    for n in readers:
        md.append(f"| cat of all input, {n} readers | {fmt(cats[(n, 'cold')])} | {fmt(cats[(n, 'warm')])} |")

    for v in variants:
        md += ["", f"## Phases of `{v.name}` (seconds, median over runs)", ""]
        cols = ["container cold", "container warm", "native warm"]
        rows = {}
        counter_rows = {}
        sources = {"container cold": stderr_runs[(v.name, "cold")], "container warm": stderr_runs[(v.name, "warm")],
                   "native warm": [(w, e) for w, _, e in native[v.name]]}
        for col, runs in sources.items():
            for wall, err in runs:
                phases, counters = parse_stderr(err)
                for k, val in phase_durations(phases, wall).items():
                    rows.setdefault(k, {}).setdefault(col, []).append(val)
                for k, val in counters.items():
                    counter_rows.setdefault(k, {}).setdefault(col, []).append(val)
        if not rows:
            md.append("(no phase log in this build)")
        else:
            md += ["| Phase | " + " | ".join(cols) + " |", "| --- |" + " --- |" * len(cols)]
            for k, per in rows.items():
                md.append(f"| {k} | " + " | ".join(
                    f"{statistics.median(per[c]):.3f}" if c in per else "-" for c in cols) + " |")
            for k, per in counter_rows.items():
                md.append(f"| cpu-s {k} (all threads) | " + " | ".join(
                    f"{statistics.median(per[c]):.3f}" if c in per else "-" for c in cols) + " |")
        md += ["", f"Native `/usr/bin/time -v` of `{v.name}` (median):", ""]
        keys = list(TIME_FIELDS.values())
        md += ["| " + " | ".join(keys) + " |", "|" + " --- |" * len(keys)]
        md.append("| " + " | ".join(
            f"{statistics.median([s.get(k, 0) for _, s, _ in native[v.name]]):.2f}" for k in keys) + " |")
    if checks:
        md += ["", "## Output checks (20M)", ""] + [f"- {c}" for c in checks]
    md += ["", "<details><summary>lscpu</summary>", "", "```", lscpu.strip(), "```", "", "</details>", ""]
    Path(a.report).write_text("\n".join(md))

    # Key numbers as annotations too.
    for v in variants:
        for mode in ("cold", "warm"):
            med, low, high = summary(container[(v.name, mode)])
            print(f"::notice title={v.name} {mode}::median {med:.3f}s (min {low:.3f}, max {high:.3f})")
        med, _, _ = summary(container[(f"{v.name} /bin/true", "warm")])
        print(f"::notice title={v.name} /bin/true warm::median {med:.3f}s")
    for c in checks:
        print(f"::notice title=output check::{c}")


if __name__ == "__main__":
    main()
