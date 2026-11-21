#!/usr/bin/env python3
"""
Copies a dataset and rewrites its user IDs (events and clicks `user_id`, both
link columns) so that they stop looking like the generator's canonical
`u` + lowercase base 36. The same original ID always becomes the same new
string in every file, so people, links and sessions are preserved.

Modes:
  all       every non-empty user ID becomes a random string that is not
            canonical: lengths 2 to 24, upper and lower case, digits, '_',
            '-', a few non-ASCII letters; byte order unrelated to the old IDs.
  tenth     a random 10% of the distinct user IDs are remapped that way, the
            rest are kept.
  variants  every ID is kept; about 1% of event, click and link rows are
            copied with the user ID upper-cased or space-padded (new events
            and clicks get fresh IDs starting with 'x'), about 0.5% of user
            ID fields are rewritten in place to such variants, and some link
            rows join a variant to its original.

Quoting is preserved, padded values are quoted, header lines, malformed and
blank lines are copied untouched, and file names stay the same.

Usage: make_adversarial.py SRC DST --mode {all,tenth,variants} [--seed N]
"""
import argparse
import hashlib
from pathlib import Path
import random
import re
import shutil

WIDTHS = {"events": 7, "clicks": 4, "links": 2}
USER_FIELDS = {"events": (1,), "clicks": (1,), "links": (0, 1)}
CANONICAL = re.compile(rb"u[0-9a-z]+\Z")

LOWER = "abcdefghijklmnopqrstuvwxyz"
UPPER = LOWER.upper()
DIGITS = "0123456789"
NON_ASCII = "éßñÅΩж中"


def parse_field(raw: bytes):
    """(ok, quoted, value) under the dialect in rules.md."""
    if b'"' not in raw:
        return True, False, raw
    if len(raw) >= 2 and raw[0:1] == b'"' and raw[-1:] == b'"' and b'"' not in raw[1:-1]:
        return True, True, raw[1:-1]
    return False, False, raw


def parse_line(line: bytes, width: int):
    """The fields as (quoted, value) pairs, or None if the line is malformed."""
    if not line:
        return None
    raws = line.split(b",")
    if len(raws) != width:
        return None
    fields = []
    for raw in raws:
        ok, quoted, value = parse_field(raw)
        if not ok:
            return None
        fields.append((quoted, value))
    return fields


def render(fields) -> bytes:
    return b",".join(b'"' + value + b'"' if quoted else value for quoted, value in fields)


def data_files(root: Path):
    for folder in WIDTHS:
        for path in sorted((root / folder).glob("*.csv")):
            yield folder, path


def split_lines(path: Path):
    """Header, body lines, and whether the file ended with a newline."""
    lines = path.read_bytes().split(b"\n")
    trailing = len(lines) > 1 and lines[-1] == b""
    if trailing:
        lines.pop()
    return lines[0], lines[1:], trailing


def join_lines(path: Path, header: bytes, body, trailing: bool):
    path.write_bytes(b"\n".join([header] + body) + (b"\n" if trailing else b""))


def collect(root: Path):
    """Distinct user IDs (in sorted order) and every event and click ID."""
    users, row_ids = set(), set()
    for folder, path in data_files(root):
        _, body, _ = split_lines(path)
        for line in body:
            fields = parse_line(line, WIDTHS[folder])
            if fields is None:
                continue
            for at in USER_FIELDS[folder]:
                if fields[at][1]:
                    users.add(fields[at][1])
            if folder != "links":
                row_ids.add(fields[0][1])
    return sorted(users), row_ids


def random_id(rng: random.Random) -> bytes:
    """A string the generator never writes as a user ID."""
    while True:
        length = rng.choice([2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15, 16, 16,
                             17, 18, 20, 22, 24])
        kind = rng.randrange(8)
        if kind == 0:    # lower case and digits, but not starting with 'u'
            text = rng.choice(LOWER.replace("u", "")) + "".join(
                rng.choice(LOWER + DIGITS) for _ in range(length - 1))
        elif kind == 1:  # canonical-looking, then upper case: like the generator's case variants
            text = "U" + "".join(rng.choice(UPPER + DIGITS) for _ in range(length - 1))
        elif kind == 2:  # mixed case and digits
            text = "".join(rng.choice(LOWER + UPPER + DIGITS) for _ in range(length))
        elif kind == 3:  # with '_' and '-'
            text = "".join(rng.choice(LOWER + UPPER + DIGITS + "_-") for _ in range(length))
        elif kind == 4:  # 'u' then one character outside [0-9a-z]: a near miss
            body = [rng.choice(LOWER + DIGITS) for _ in range(length - 1)]
            body[rng.randrange(len(body))] = rng.choice(UPPER + "_-")
            text = "u" + "".join(body)
        elif kind == 5:  # digits only
            text = "".join(rng.choice(DIGITS) for _ in range(length))
        elif kind == 6:  # starts with '-' or '_'
            text = rng.choice("_-") + "".join(rng.choice(LOWER + DIGITS) for _ in range(length - 1))
        else:            # a non-ASCII letter somewhere (multi-byte UTF-8)
            chars = [rng.choice(LOWER + UPPER + DIGITS) for _ in range(length)]
            chars[rng.randrange(length)] = rng.choice(NON_ASCII)
            text = "".join(chars)
        value = text.encode()
        if not CANONICAL.match(value):
            return value


def remap(users, share: float, seed: int):
    """original -> new string for a share of the distinct IDs, all new strings unique."""
    rng = random.Random(f"remap-{seed}")
    taken = set(users)
    mapping = {}
    for user in users:
        if share < 1 and rng.random() >= share:
            continue
        while True:
            new = random_id(rng)
            if new not in taken:
                break
        taken.add(new)
        mapping[user] = new
    return mapping


def rewrite(dst: Path, mapping):
    for folder, path in data_files(dst):
        header, body, trailing = split_lines(path)
        for i, line in enumerate(body):
            fields = parse_line(line, WIDTHS[folder])
            if fields is None:
                continue
            changed = False
            for at in USER_FIELDS[folder]:
                quoted, value = fields[at]
                if value in mapping:
                    fields[at] = (quoted, mapping[value])
                    changed = True
            if changed:
                body[i] = render(fields)
        join_lines(path, header, body, trailing)


def variant(rng: random.Random, value: bytes) -> bytes:
    """An upper-cased or space-padded form of an ID, never the ID itself."""
    kind = rng.randrange(5)
    if kind == 0 and value.upper() != value:
        return value.upper()
    if kind == 1 and value.upper() != value:
        return b" " + value.upper()
    pad = b" " * rng.choice([1, 1, 1, 2])
    side = rng.randrange(3)
    return (pad if side != 1 else b"") + value + (pad if side != 0 else b"")


def put(quoted, value):
    """A field holding value: quoted if the source was or if it has spaces."""
    return (quoted or value.startswith(b" ") or value.endswith(b" "), value)


def add_variants(dst: Path, seed: int, row_ids):
    assert not any(i[:1] in (b"x", b"X") for i in row_ids), "existing IDs start with x"
    fresh = 0
    pairs = []
    link_files = []
    stats = {"copies": 0, "in_place": 0, "variant_links": 0}
    for folder, path in data_files(dst):
        rng = random.Random(f"variants-{seed}-{folder}-{path.name}")
        header, body, trailing = split_lines(path)
        if folder == "links":
            link_files.append(path)
        added = []
        for i, line in enumerate(body):
            fields = parse_line(line, WIDTHS[folder])
            if fields is None:
                continue
            users = [at for at in USER_FIELDS[folder] if fields[at][1]]
            if not users:
                continue
            # Copy about 1% of rows with a variant user ID.
            if rng.random() < 0.01:
                copy = list(fields)
                sides = users if folder != "links" or rng.random() < 0.2 else [rng.choice(users)]
                for at in sides:
                    quoted, value = copy[at]
                    new = variant(rng, value)
                    copy[at] = put(quoted, new)
                    pairs.append((new, value))
                if folder != "links":
                    fresh += 1
                    copy[0] = (copy[0][0], b"x%d" % fresh)
                added.append(render(copy))
                stats["copies"] += 1
            # Rewrite about 0.5% of user ID fields in place. Events and clicks
            # decide by their own ID and value, so every copy of a duplicated
            # click (whose copies must stay identical) gets the same variant.
            changed = False
            for at in users:
                quoted, value = fields[at]
                pick = rng if folder == "links" else random.Random(
                    b"in-place-%d-%s-%s" % (seed, fields[0][1], value))
                if pick.random() < 0.005:
                    new = variant(pick, value)
                    fields[at] = put(quoted, new)
                    pairs.append((new, value))
                    changed = True
            if changed:
                body[i] = render(fields)
                stats["in_place"] += 1
        for line in added:
            body.insert(rng.randint(0, len(body)), line)
        join_lines(path, header, body, trailing)

    # Link about a third of the variants back to their originals.
    rng = random.Random(f"variant-links-{seed}")
    joins = {}
    for new, old in pairs:
        if rng.random() < 0.33:
            joins.setdefault(link_files[rng.randrange(len(link_files))], []).append(
                [put(rng.random() < 0.2, new), (rng.random() < 0.2, old)][::rng.choice([1, -1])])
    for path, rows in joins.items():
        header, body, trailing = split_lines(path)
        for row in rows:
            body.insert(rng.randint(0, len(body)), render(row))
        join_lines(path, header, body, trailing)
        stats["variant_links"] += len(rows)
    return stats


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("src", type=Path)
    parser.add_argument("dst", type=Path)
    parser.add_argument("--mode", required=True, choices=["all", "tenth", "variants"])
    parser.add_argument("--seed", type=int, default=1)
    args = parser.parse_args()
    if args.dst.exists():
        shutil.rmtree(args.dst)
    shutil.copytree(args.src, args.dst)
    users, row_ids = collect(args.dst)
    seed = int.from_bytes(hashlib.blake2b(f"{args.mode}-{args.seed}".encode(), digest_size=4).digest(), "big")
    if args.mode == "variants":
        stats = add_variants(args.dst, seed, row_ids)
        print(f"{len(users)} user IDs kept; {stats}")
    else:
        mapping = remap(users, 1.0 if args.mode == "all" else 0.1, seed)
        rewrite(args.dst, mapping)
        print(f"{len(mapping)} of {len(users)} user IDs remapped")


if __name__ == "__main__":
    main()
