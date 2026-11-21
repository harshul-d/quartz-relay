#!/usr/bin/env python3
"""
Shopping Sessions: slow, plain reference implementation (the oracle).

It follows rules.md line by line and favours clarity over speed. The Rust
pipeline is tested against it.

Reads  $DATA_DIR   (or the next-to-last argument), default /data
Writes $OUTPUT_DIR (or the last argument), default /output:
       user_daily/part-0.parquet and campaign_daily/part-0.parquet

Only the standard library is used, plus pyarrow to write Parquet. The CSV
dialect is parsed by hand because no library reader matches rules.md.
"""
import datetime
import json
import os
import sys

import pyarrow as pa
import pyarrow.parquet as pq

SECOND = 1_000
MINUTE = 60 * SECOND
HOUR = 60 * MINUTE
DAY = 24 * HOUR

SESSION_GAP = 30 * MINUTE        # Events 5: a gap over 30 minutes starts a session
LATE_LIMIT = 24 * HOUR           # Events 4: ingest_ts - ts over 24 hours is late
CLICK_LOOKBACK = 7 * DAY         # Clicks 4 and 7: clicks up to 7 days before a session
MAX_PRICE = 10_000_000           # Events 3: purchase price from 1 to 10,000,000

DIGITS = b"0123456789"           # "digits" always means ASCII 0-9


# ---------------------------------------------------------------------------
# CSV dialect (rules.md "CSV dialect")
# ---------------------------------------------------------------------------

def parse_field(raw):
    """One field: returns (ok, value). Quotes are syntax only around a whole
    field; a quote anywhere else makes the line malformed. An empty value,
    quoted or not, is null (None). Spaces and \\r are ordinary characters."""
    if b'"' in raw:
        # The only allowed quotes are one at the start and one at the end.
        if len(raw) < 2 or raw[0:1] != b'"' or raw[-1:] != b'"' or b'"' in raw[1:-1]:
            return False, None
        raw = raw[1:-1]
    return True, (raw if raw else None)


def parse_line(line, field_count):
    """A line split on every comma (no value ever holds a comma). Returns the
    list of values, or None if the line is malformed: wrong field count, a
    stray quote, or empty (an empty line has one empty field, which is the
    wrong count for every table here, and is malformed in any case)."""
    if line == b"":
        return None
    raw_fields = line.split(b",")
    if len(raw_fields) != field_count:
        return None
    values = []
    for raw in raw_fields:
        ok, value = parse_field(raw)
        if not ok:
            return None
        values.append(value)
    return values


def read_csv(path, field_count):
    """Yields the parsed data rows of one file. Only \\n ends a line. The first
    line is the header and is skipped; a later line equal to the header is data.
    Malformed lines are dropped."""
    with open(path, "rb") as f:
        data = f.read()
    lines = data.split(b"\n")
    # A final \n terminates the last line; it does not start an empty one.
    if lines and lines[-1] == b"":
        lines.pop()
    for line in lines[1:]:
        values = parse_line(line, field_count)
        if values is not None:
            yield values


def csv_files(folder):
    """Every *.csv file in the folder (rules.md "Input files")."""
    if not os.path.isdir(folder):
        return []
    names = sorted(n for n in os.listdir(folder) if n.endswith(".csv") and not n.startswith("."))
    return [os.path.join(folder, n) for n in names if os.path.isfile(os.path.join(folder, n))]


# ---------------------------------------------------------------------------
# Timestamps (rules.md "Timestamps")
# ---------------------------------------------------------------------------

def is_leap(year):
    return year % 4 == 0 and (year % 100 != 0 or year % 400 == 0)


def days_in_month(year, month):
    if month == 2:
        return 29 if is_leap(year) else 28
    return 30 if month in (4, 6, 9, 11) else 31


def days_from_civil(year, month, day):
    """Days since 1970-01-01 of a proleptic Gregorian date."""
    return (datetime.date(year, month, day) - datetime.date(1970, 1, 1)).days


def all_digits(b):
    return len(b) > 0 and all(c in DIGITS for c in b)


def parse_civil(b):
    """`YYYY-MM-DDTHH:MM:SS` (19 bytes) -> epoch ms as if UTC, or None."""
    if len(b) != 19:
        return None
    if b[4:5] != b"-" or b[7:8] != b"-" or b[10:11] != b"T" or b[13:14] != b":" or b[16:17] != b":":
        return None
    parts = (b[0:4], b[5:7], b[8:10], b[11:13], b[14:16], b[17:19])
    if not all(all_digits(p) for p in parts):
        return None
    year, month, day, hour, minute, second = (int(p) for p in parts)
    if year < 1 or not 1 <= month <= 12:          # month 13 is impossible
        return None
    if not 1 <= day <= days_in_month(year, month):  # Feb 30, Feb 29 outside leap years
        return None
    if hour > 23 or minute > 59 or second > 59:   # hour 24, second 60
        return None
    days = days_from_civil(year, month, day)
    return ((days * 24 + hour) * 60 + minute) * MINUTE + second * SECOND


def parse_ts(b):
    """A timestamp in form F1, F2 or F3 -> integer epoch ms, or None if bad.
    F3 values of any length are numbers (Python ints are unbounded), so a
    value too long for 64 bits simply falls outside every window."""
    if b is None:                                   # empty is bad
        return None
    # F3: digits only, epoch milliseconds, leading zeros allowed.
    if all_digits(b):
        return int(b)
    # F1: 2026-10-03T23:58:10Z (uppercase Z only)
    if len(b) == 20 and b[19:20] == b"Z":
        return parse_civil(b[:19])
    # F2: 2026-10-03T19:58:10-04:00, offset hours 00-23, minutes 00-59, with colon
    if len(b) == 25 and b[19:20] in (b"+", b"-") and b[22:23] == b":":
        local = parse_civil(b[:19])
        oh, om = b[20:22], b[23:25]
        if local is None or not all_digits(oh) or not all_digits(om):
            return None
        oh, om = int(oh), int(om)
        if oh > 23 or om > 59:
            return None
        offset = (oh * 60 + om) * MINUTE
        # Subtract the offset to get UTC. -00:00 is UTC.
        return local - offset if b[19:20] == b"+" else local + offset
    return None


def parse_price(b):
    """A purchase price: digits only, value 1 to 10,000,000, else None."""
    if b is None or not all_digits(b):
        return None
    value = int(b)                                  # "0250" is 250
    return value if 1 <= value <= MAX_PRICE else None


# ---------------------------------------------------------------------------
# Linked IDs (rules.md "Linked IDs")
# ---------------------------------------------------------------------------

class UnionFind:
    def __init__(self):
        self.parent = {}

    def find(self, x):
        root = x
        while self.parent[root] != root:
            root = self.parent[root]
        while self.parent[x] != root:               # path compression
            self.parent[x], x = root, self.parent[x]
        return root

    def add(self, x):
        if x not in self.parent:
            self.parent[x] = x

    def union(self, a, b):
        self.add(a)
        self.add(b)
        ra, rb = self.find(a), self.find(b)
        if ra != rb:
            self.parent[ra] = rb


def build_person_names(data_dir):
    """Maps every linked ID to its person's name: the byte-wise smallest ID."""
    uf = UnionFind()
    for path in csv_files(os.path.join(data_dir, "links")):
        for a, b in read_csv(path, 2):
            if a is None or b is None:              # Linked IDs 1: an empty side drops the line
                continue
            uf.union(a, b)                          # Linked IDs 2: transitive joining
    smallest = {}
    for x in uf.parent:
        root = uf.find(x)
        if root not in smallest or x < smallest[root]:   # bytes compare byte-wise
            smallest[root] = x
    return {x: smallest[uf.find(x)] for x in uf.parent}


# ---------------------------------------------------------------------------
# Pipeline
# ---------------------------------------------------------------------------

def main():
    args = sys.argv[1:]
    data_dir = os.environ.get("DATA_DIR") or (args[-2] if len(args) >= 2 else "/data")
    out_dir = os.environ.get("OUTPUT_DIR") or (args[-1] if len(args) >= 2 else "/output")

    with open(os.path.join(data_dir, "manifest.json"), "rb") as f:
        manifest = json.load(f)
    window_start = parse_ts(manifest["window_start"].encode())
    window_end = parse_ts(manifest["window_end"].encode())
    assert window_start is not None and window_end is not None, "bad manifest"
    click_window_start = window_start - CLICK_LOOKBACK

    # Lookups: products.csv and campaigns.csv are clean.
    # (Rows with an empty field are skipped; a repeated ID keeps its last row.)
    category_of = {}
    for product_id, category in read_csv(os.path.join(data_dir, "products.csv"), 2):
        if product_id is not None and category is not None:
            category_of[product_id] = category
    channel_of = {}
    for campaign_id, channel in read_csv(os.path.join(data_dir, "campaigns.csv"), 2):
        if campaign_id is not None and channel is not None:
            channel_of[campaign_id] = channel

    # Linked IDs: an ID in no remaining link is a person on its own.
    names = build_person_names(data_dir)

    def person(user_id):
        return names.get(user_id, user_id)

    # ---- Events 1-2: parse, then dedup on event_id by smallest ingest_ts ----
    kept = {}   # event_id -> (ingest_ms, row)
    for path in csv_files(os.path.join(data_dir, "events")):
        for row in read_csv(path, 7):
            event_id = row[0]
            if event_id is None:                    # Events 1: empty event_id
                continue
            ingest = parse_ts(row[3])
            if ingest is None:
                # rules.md: ingest_ts is always valid F1. Should it not be, the
                # copy sorts after every valid copy (a documented choice).
                ingest = float("inf")
            prev = kept.get(event_id)
            if prev is None or ingest < prev[0]:    # copies with equal ingest_ts are identical
                kept[event_id] = (ingest, row)

    # ---- Events 3-4: validate the kept copy, drop late events ----
    events_by_person = {}   # person -> list of (ts, is_purchase, price, category)
    for ingest, row in kept.values():
        _, user_id, ts_raw, _, event_type, product_id, price_raw = row
        if user_id is None or product_id is None:
            continue
        ts = parse_ts(ts_raw)
        if ts is None or not (window_start <= ts < window_end):
            continue
        if event_type not in (b"view", b"cart", b"purchase"):
            continue
        price = 0
        if event_type == b"purchase":
            price = parse_price(price_raw)
            if price is None:
                continue
        if ingest - ts > LATE_LIMIT:                # Events 4: exactly 24 h is kept
            continue
        category = category_of.get(product_id, b"unknown")   # Events 7
        events_by_person.setdefault(person(user_id), []).append(
            (ts, event_type == b"purchase", price, category))

    # ---- Clicks 1-3: parse, dedup on click_id, validate ----
    clicks_by_id = {}
    for path in csv_files(os.path.join(data_dir, "clicks")):
        for row in read_csv(path, 4):
            click_id = row[0]
            if click_id is None:                    # Clicks 1
                continue
            if click_id not in clicks_by_id:        # Clicks 2: copies are identical, keep any
                clicks_by_id[click_id] = row
    clicks_by_person = {}   # person -> list of (ts, click_id, campaign_id)
    for click_id, user_id, campaign_id, ts_raw in clicks_by_id.values():
        if user_id is None:
            continue
        ts = parse_ts(ts_raw)
        if ts is None or not (click_window_start <= ts < window_end):
            continue
        if campaign_id not in channel_of:
            continue
        clicks_by_person.setdefault(person(user_id), []).append((ts, click_id, campaign_id))

    # ---- Events 5-6, Clicks 4-7: sessionize, credit, assist, aggregate ----
    user_daily = {}       # (person, day) -> [sessions, events, revenue, {category: cents}]
    campaign_daily = {}   # (campaign, day) -> [sessions, purchasing, revenue, assisted]

    def campaign_row(campaign, day):
        return campaign_daily.setdefault((campaign, day), [0, 0, 0, 0])

    for name, events in events_by_person.items():
        # Sort by ts only; events sharing a ts always fall in one session.
        events.sort(key=lambda e: e[0])
        sessions = []
        for event in events:
            if not sessions or event[0] - sessions[-1][-1][0] > SESSION_GAP:
                sessions.append([event])
            else:
                sessions[-1].append(event)

        clicks = clicks_by_person.get(name, [])
        for session in sessions:
            start = session[0][0]
            day = start // DAY                      # Events 6: UTC date of the start
            n_events = len(session)
            revenue = sum(e[2] for e in session if e[1])
            purchased = any(e[1] for e in session)

            row = user_daily.setdefault((name, day), [0, 0, 0, {}])
            row[0] += 1
            row[1] += n_events
            row[2] += revenue
            for _, is_purchase, price, category in session:
                if is_purchase:
                    row[3][category] = row[3].get(category, 0) + price

            # Clicks 4 and 7: the person's clicks with start - 7 days <= ts <= start.
            qualifying = [c for c in clicks if start - CLICK_LOOKBACK <= c[0] <= start]
            if qualifying:
                # Latest ts; on a tie the smallest click_id, byte by byte.
                best = min(qualifying, key=lambda c: (-c[0], c[1]))
                credited = best[2]
                for campaign in set(c[2] for c in qualifying):
                    campaign_row(campaign, day)[3] += 1
            else:
                credited = b"organic"               # Clicks 5
            crow = campaign_row(credited, day)
            crow[0] += 1
            crow[1] += 1 if purchased else 0
            crow[2] += revenue

    # ---- Outputs ----
    epoch = datetime.date(1970, 1, 1)

    def to_date(day):
        return epoch + datetime.timedelta(days=day)

    def top_category(cats):
        if not cats:
            return None
        best = max(cats.values())
        return min(c for c, v in cats.items() if v == best).decode()

    ud = sorted(user_daily.items())
    user_table = pa.table({
        "user_id": pa.array([k[0].decode() for k, _ in ud], pa.string()),
        "day": pa.array([to_date(k[1]) for k, _ in ud], pa.date32()),
        "sessions": pa.array([v[0] for _, v in ud], pa.int64()),
        "events": pa.array([v[1] for _, v in ud], pa.int64()),
        "revenue_cents": pa.array([v[2] for _, v in ud], pa.int64()),
        "top_category": pa.array([top_category(v[3]) for _, v in ud], pa.string()),
    })
    cd = sorted(campaign_daily.items())
    campaign_table = pa.table({
        "campaign_id": pa.array([k[0].decode() for k, _ in cd], pa.string()),
        "channel": pa.array([(b"organic" if k[0] == b"organic" else channel_of[k[0]]).decode()
                             for k, _ in cd], pa.string()),
        "day": pa.array([to_date(k[1]) for k, _ in cd], pa.date32()),
        "sessions": pa.array([v[0] for _, v in cd], pa.int64()),
        "purchasing_sessions": pa.array([v[1] for _, v in cd], pa.int64()),
        "revenue_cents": pa.array([v[2] for _, v in cd], pa.int64()),
        "assisted_sessions": pa.array([v[3] for _, v in cd], pa.int64()),
    })
    for table_name, table in (("user_daily", user_table), ("campaign_daily", campaign_table)):
        folder = os.path.join(out_dir, table_name)
        os.makedirs(folder, exist_ok=True)
        pq.write_table(table, os.path.join(folder, "part-0.parquet"))
    print(f"user_daily {user_table.num_rows} rows, campaign_daily {campaign_table.num_rows} rows",
          file=sys.stderr)


if __name__ == "__main__":
    main()
