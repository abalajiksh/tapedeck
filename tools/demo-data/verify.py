#!/usr/bin/env python3
"""Audit a generated demo history the way Tapedeck will actually read it.

Reimplements the rules that decide whether a listen survives the import, rather
than checking the file is well-formed JSON — which it trivially is. Three of
these have bitten this codebase for real:

* `listen_to_play` needs `listened_at`, `track_name` and `artist_name`; a row
  missing any of them is dropped silently, with nothing logged.
* `fuzzy_exists` drops a second play of the same track within a window of
  `clamp(duration * 4/5, 20, 60)` seconds. A back-to-back repeat that lands
  inside it disappears, which is exactly the bug v0.13.3 fixed for Plex.
* A future timestamp is the anomaly every audit of a real export checks for.

Then it prints the shape of the history, because "it imported" and "it makes
the application look like anything" are different claims.
"""

import collections
import datetime as dt
import json
import sys
from pathlib import Path

UTC = dt.timezone.utc


def main(path):
    rows = json.loads(Path(path).read_text(encoding="utf-8"))
    now = int(dt.datetime.now(UTC).timestamp())

    problems = []

    # ── what the parser requires ──────────────────────────────────────────
    parsed = []
    for i, r in enumerate(rows):
        ts = r.get("listened_at")
        tm = r.get("track_metadata", r)
        title, artist = tm.get("track_name"), tm.get("artist_name")
        if ts is None or not title or not artist:
            problems.append(f"row {i}: would be dropped by listen_to_play")
            continue
        info = tm.get("additional_info", {})
        parsed.append(
            (
                ts,
                title,
                artist,
                tm.get("release_name"),
                info.get("duration_ms", 0) // 1000,
                info.get("tracknumber"),
            )
        )
        if ts > now:
            problems.append(f"row {i}: timestamp is in the future")

    # ── what fuzzy_exists would eat ───────────────────────────────────────
    last_seen = {}
    eaten = 0
    for ts, title, artist, _album, secs, _n in sorted(parsed):
        window = min(max(secs * 4 // 5, 20), 60) if secs else 60
        key = (artist, title)
        if key in last_seen and ts - last_seen[key] <= window:
            eaten += 1
        last_seen[key] = ts
    if eaten:
        problems.append(f"{eaten} listens would be dropped as duplicates by fuzzy_exists")

    # ── sessions, as assign_session would group them ──────────────────────
    sessions, cur = [], 1
    ordered = sorted(parsed)
    for a, b in zip(ordered, ordered[1:]):
        if b[0] - a[0] > 30 * 60:
            sessions.append(cur)
            cur = 1
        else:
            cur += 1
    sessions.append(cur)

    # ── the shape ─────────────────────────────────────────────────────────
    first = dt.datetime.fromtimestamp(ordered[0][0], UTC)
    last = dt.datetime.fromtimestamp(ordered[-1][0], UTC)
    days = (last.date() - first.date()).days + 1
    hours = sum(r[4] for r in parsed) / 3600
    per_year = collections.Counter(dt.datetime.fromtimestamp(r[0], UTC).year for r in parsed)
    per_hour = collections.Counter(dt.datetime.fromtimestamp(r[0], UTC).hour for r in parsed)
    artists = collections.Counter(r[2] for r in parsed)
    albums = collections.Counter((r[2], r[3]) for r in parsed)
    active_days = len({dt.datetime.fromtimestamp(r[0], UTC).date() for r in parsed})

    print(f"{len(parsed):,} listens   {first.date()} → {last.date()}   ({days} days)")
    print(f"{len(artists)} artists   {len(albums)} records   {hours:,.0f} listening hours")
    print(f"{len(parsed) / days:.1f} per calendar day   {active_days} days with any listening")
    print(f"{len(sessions):,} sessions, median {sorted(sessions)[len(sessions) // 2]} tracks")
    print(f"{sum(1 for r in parsed if r[5]):,} carry a track number, "
          f"{sum(1 for r in parsed if r[4]):,} a duration")

    print("\nper year")
    for y in sorted(per_year):
        print(f"  {y}  {per_year[y]:>6,}  {'█' * (per_year[y] // 120)}")

    print("\nby hour (UTC)")
    peak = max(per_hour.values())
    for h in range(24):
        n = per_hour.get(h, 0)
        print(f"  {h:02d}  {n:>5,}  {'█' * round(28 * n / peak)}")

    print("\ntop 12 artists")
    for a, n in artists.most_common(12):
        print(f"  {n:>5,}  {a}")

    # A crush should be visible as a spike, not a rumour.
    print("\nbiggest single artist-month (the Reports crush should find these)")
    am = collections.Counter(
        (r[2], dt.datetime.fromtimestamp(r[0], UTC).strftime("%Y-%m")) for r in parsed
    )
    for (a, m), n in am.most_common(6):
        print(f"  {n:>4}  {a} in {m}")

    print()
    if problems:
        for p in problems[:20]:
            print("PROBLEM:", p)
        print(f"{len(problems)} problem(s)")
        return 1
    print("No problems: every listen survives listen_to_play, fuzzy_exists and the clock.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "tapedeck-demo-history.json"))
