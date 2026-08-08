#!/usr/bin/env python3
"""Generate eight years of demo listening history for Tapedeck.

Output is a JSON array of ListenBrainz-shaped listen objects — the exact shape
`server/import.rs::listen_to_play` reads — so it goes in through
Settings → Import → *Import from file* with nothing else set up.

Why JSON and not CSV: `parse_csv` keeps only title/artist/album/mbid/time. The
duration and the track number are dropped, and those two are what feed session
grouping, listening hours, the per-day average and an album's running order. A
CSV demo history renders about half the application empty.

Why no MBIDs: a fabricated recording or release id would be written into
`mb_cache` and resolve the wrong cover permanently, and a fabricated `caa_id`
points at nothing. With none of them present Tapedeck looks each record up the
way it would for any real listen — which also means the artwork chain and the
MusicBrainz backfill are themselves part of what the demo demonstrates.

The simulation aims at the things this application actually draws:

* **A day has a shape.** Sessions land in a morning block, an afternoon block,
  an evening block and — rarely — the small hours, so the hour-of-day heatmap
  and the Reports clock have something in them, and the 3 AM Club collection is
  not empty.
* **Sessions are contiguous.** Tracks inside one run back-to-back; consecutive
  sessions are pushed at least 35 minutes apart, which is the far side of
  `assign_session`'s 30-minute gap. Rebuilding sessions therefore finds real
  ones rather than one session per listen.
* **Records get played as records.** Just under half of all sessions are an
  album run from track one, in order, which is what makes an album page look
  like a record.
* **Taste moves.** Every artist carries an era profile, so the genre-map
  trajectory has a path to draw and the Reports "crush" has a month where
  somebody genuinely arrived.
* **Some things get dropped.** A few artists are heavy early and gone by the
  end, which is the whole premise of the Rediscover screen.

Usage:

    python3 generate.py                        # ~8 years to tapedeck-demo-history.json
    python3 generate.py --years 6 --out foo.json
    python3 generate.py --seed 7               # a different, reproducible history

Deterministic: the same seed gives the same file, byte for byte.
"""

import argparse
import datetime as dt
import json
import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from catalog import (  # noqa: E402
    ALBUMS,
    ARTISTS,
    EXPLORE,
    NOCTURNAL,
    TIER_WEIGHT,
    tier_of,
)

# Timestamps are epoch seconds, laid out against a UTC wall clock. Tapedeck
# buckets hours, "today" and the trajectory in the *user's* configured zone, so
# a demo instance set to UTC sees the day shape exactly as generated; any other
# zone shifts it by that offset, which is harmless but worth knowing.
EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)

# The last day generated: yesterday, so a freshly generated history runs right
# up to now. Never today or later — a listen with a future timestamp is the
# anomaly every audit of this codebase checks for, and `import_is_live` treats
# one as clock skew.
#
# The shipped file is therefore only as current as the day it was generated. A
# regenerate is one command, which is most of why the generator ships too.
DEFAULT_END = dt.date.today() - dt.timedelta(days=1)


def gauss(t, mu, sigma):
    return math.exp(-((t - mu) ** 2) / (2 * sigma * sigma))


def build_profiles(span_years):
    """artist -> f(t in years, month) -> relative weight."""

    def steady(phase):
        return lambda t, m: 0.75 + 0.25 * math.sin(t * 1.1 + phase)

    def early(t, m):
        return 0.12 + 1.15 * gauss(t, span_years * 0.14, span_years * 0.20)

    def mid(t, m):
        return 0.12 + 1.15 * gauss(t, span_years * 0.50, span_years * 0.22)

    def late(t, m):
        return 0.08 + 1.20 * gauss(t, span_years * 0.86, span_years * 0.20)

    def fade(t, m):
        return max(0.02, 1.10 - t / (span_years * 0.45))

    def revival(t, m):
        return 0.08 + gauss(t, span_years * 0.10, span_years * 0.13) + gauss(
            t, span_years * 0.82, span_years * 0.15
        )

    def wave(phase):
        return lambda t, m: 0.30 + 0.70 * (0.5 + 0.5 * math.sin(t * 2.0 + phase))

    def crush(when):
        """Discovered in one month, hammered, then settles into the rotation.

        Zero before the discovery month, and that is the whole point rather than
        a detail. Reports' crush is "the most-played artist you had never played
        before" (`reports.rs`), so an artist carrying even a small baseline from
        day one is never new and can never be a crush — the first version of
        this generator gave them 0.10 from the start and the crush card came
        back `null` for all 97 months.
        """
        y, mo = (int(x) for x in when.split("-"))
        centre = (dt.date(y, mo, 15) - START).days / 365.25

        def f(t, m):
            if t < centre - 0.04:
                return 0.0
            return 0.10 + 3.6 * gauss(t, centre, 0.13) + 0.55 * gauss(t, centre + 0.55, 0.75)

        return f

    def xmas(t, m):
        # Nothing at all for eleven months of the year, then a real presence in
        # December — but not an takeover. Set high enough, these three ended up
        # the biggest artist-months in the whole history, which put a Christmas
        # record where the Reports crush should be naming a genuine discovery.
        return 3.5 if m == 12 else 0.004

    shapes = {
        "early": early, "mid": mid, "late": late,
        "fade": fade, "revival": revival, "xmas": xmas,
    }

    profiles = {}
    for i, (artist, (era, _lang)) in enumerate(ARTISTS.items()):
        if era == "steady":
            shape = steady(i * 0.7)
        elif era == "wave":
            shape = wave(i * 0.9)
        elif era.startswith("crush:"):
            shape = crush(era.split(":", 1)[1])
        elif era in shapes:
            shape = shapes[era]
        else:
            raise SystemExit(f"unknown era profile {era!r} for {artist}")

        # The era decides *when*; the tier decides *how much*. Keeping them
        # separate is what lets a `core` artist fade and come back while still
        # being the biggest thing in the history either side of the gap.
        scale = TIER_WEIGHT[tier_of(artist)]
        profiles[artist] = (lambda s, k: lambda t, m: k * s(t, m))(shape, scale)
    return profiles


# Hour-of-day blocks: (start, end, weight). Deliberately not uniform — a flat
# clock face is the giveaway that a history was generated rather than lived.
BLOCKS = [
    (7, 10, 22),    # morning, getting going
    (10, 13, 14),   # late morning
    (13, 17, 18),   # afternoon
    (17, 20, 12),   # evening, cooking
    (20, 24, 28),   # the main listening block
    (0, 3, 4),      # the 3 AM Club: rare, and real
    (3, 7, 2),      # very early, rarer still
]


def pick_hour(rnd, weekend):
    blocks = BLOCKS
    if weekend:
        # Weekends start later and run longer.
        blocks = [(a, b, w * (1.6 if a >= 10 else 0.6)) for a, b, w in BLOCKS]
    total = sum(w for _, _, w in blocks)
    r = rnd.uniform(0, total)
    for a, b, w in blocks:
        if r < w:
            return rnd.randrange(a, b)
        r -= w
    return 21


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--years", type=float, default=8.0, help="how far back to start (default 8)")
    ap.add_argument("--seed", type=int, default=1974)
    ap.add_argument("--out", default="tapedeck-demo-history.json")
    ap.add_argument("--end", default=DEFAULT_END.isoformat(), help="last day, YYYY-MM-DD")
    args = ap.parse_args()

    global START
    end = dt.date.fromisoformat(args.end)
    if end >= dt.date.today():
        raise SystemExit(
            f"--end {end} is today or later; a listen in the future is bad demo data"
        )
    START = end - dt.timedelta(days=round(args.years * 365.25))
    span_years = (end - START).days / 365.25

    rnd = random.Random(args.seed)
    profiles = build_profiles(span_years)

    # Index the catalogue: albums per artist, and the flat track pool.
    by_artist = {}
    for artist, album, tracks in ALBUMS:
        if artist not in ARTISTS:
            raise SystemExit(f"{artist!r} has an album but no era profile")
        by_artist.setdefault(artist, []).append((album, tracks))

    # The exploration queue: every track in EXPLORE, scheduled once, on a day
    # drawn uniformly across the whole span.
    #
    # Drawing these *with* replacement — a flat 9% of shuffle picks — was the
    # obvious version and produced a one-play tail of exactly zero, because a
    # pool of 160 tracks sampled 1,700 times gives every one of them about ten
    # plays. Scheduling each exactly once is what makes them one-play tracks,
    # which is the entire population `db::heard_once` selects from.
    total_days = (end - START).days + 1
    explore_pool = [
        (artist, album, i + 1, title, secs)
        for artist, album, tracks in EXPLORE
        for i, (title, secs) in enumerate(tracks)
    ]
    rnd.shuffle(explore_pool)
    explore_by_day = {}
    for item in explore_pool:
        explore_by_day.setdefault(rnd.randrange(0, total_days), []).append(item)
    pending = []

    listens = []
    day = START
    while day <= end:
        t = (day - START).days / 365.25
        weekend = day.weekday() >= 5
        pending.extend(explore_by_day.get((day - START).days, []))

        weights = {a: max(0.0, f(t, day.month)) for a, f in profiles.items()}
        live = [
            (a, w)
            for a, w in weights.items()
            if w > 0.01 and a in by_artist and a not in NOCTURNAL
        ]
        night = [(a, weights.get(a, 1.0) or 1.0) for a in NOCTURNAL if a in by_artist]
        if not live:
            day += dt.timedelta(days=1)
            continue

        # How much listening happened at all. A real history has quiet days and
        # the occasional day off entirely.
        roll = rnd.random()
        if roll < (0.06 if weekend else 0.11):
            n_sessions = 0
        elif roll < 0.45:
            n_sessions = 1
        elif roll < 0.80:
            n_sessions = 2
        else:
            n_sessions = 3 if not weekend else rnd.choice([3, 4])

        starts = sorted(pick_hour(rnd, weekend) for _ in range(n_sessions))
        cursor = None
        for hour in starts:
            base = dt.datetime(day.year, day.month, day.day, hour, tzinfo=dt.timezone.utc)
            start_ts = int((base - EPOCH).total_seconds()) + rnd.randrange(0, 3600)
            # Sessions must land on the far side of the 30-minute gap that
            # `assign_session` breaks on, or the whole day folds into one.
            if cursor is not None and start_ts < cursor + 35 * 60:
                start_ts = cursor + 35 * 60 + rnd.randrange(0, 40 * 60)

            length = max(3, min(26, int(rnd.gauss(9, 5))))
            ts = start_ts

            # In the small hours it is the ambient shelf, and only that. A track
            # reaches the 3 AM Club by having 70% of its plays land between
            # midnight and 5am, which nothing can do while sharing a pool with
            # the daytime rotation.
            nocturnal = hour < 5 and night and rnd.random() < 0.8
            pool = night if nocturnal else live

            if rnd.random() < 0.45:
                # An album, played as one.
                artist = weighted_choice(rnd, pool)
                album, tracks = rnd.choice(by_artist[artist])
                first = 0 if rnd.random() < 0.72 else rnd.randrange(0, len(tracks))
                run = tracks[first:first + length]
                for i, (title, secs) in enumerate(run):
                    listens.append(listen(ts, title, artist, album, secs, first + i + 1))
                    ts += secs + rnd.randrange(0, 7)
            else:
                # A shuffle across whatever is in rotation this month.
                prev = None
                for _ in range(length):
                    if prev and rnd.random() < 0.06:
                        title, artist, album, secs, num = prev  # played twice
                    elif pending and not nocturnal and rnd.random() < 0.4:
                        # Something tried once out of the wider catalogue. This
                        # is where the one-play tail comes from, and without it
                        # a 35,000-listen history has 678 unique tracks in it.
                        artist, album, num, title, secs = pending.pop()
                    else:
                        artist = weighted_choice(rnd, pool)
                        album, tracks = rnd.choice(by_artist[artist])
                        idx = rnd.randrange(0, len(tracks))
                        title, secs = tracks[idx]
                        num = idx + 1
                    listens.append(listen(ts, title, artist, album, secs, num))
                    prev = (title, artist, album, secs, num)
                    ts += secs + rnd.randrange(0, 9)

            cursor = ts

        day += dt.timedelta(days=1)

    listens.sort(key=lambda l: l["listened_at"])

    out = Path(args.out)
    with out.open("w", encoding="utf-8") as fh:
        fh.write("[\n")
        for i, l in enumerate(listens):
            fh.write(json.dumps(l, ensure_ascii=False, separators=(",", ":")))
            fh.write(",\n" if i < len(listens) - 1 else "\n")
        fh.write("]\n")

    first = dt.datetime.fromtimestamp(listens[0]["listened_at"], dt.timezone.utc).date()
    last = dt.datetime.fromtimestamp(listens[-1]["listened_at"], dt.timezone.utc).date()
    artists = {l["track_metadata"]["artist_name"] for l in listens}
    albums = {
        (l["track_metadata"]["artist_name"], l["track_metadata"]["release_name"]) for l in listens
    }
    print(f"{len(listens):,} listens  {first} → {last}")
    print(f"{len(artists)} artists, {len(albums)} records, {out.stat().st_size / 1e6:.1f} MB")


def weighted_choice(rnd, pairs):
    total = sum(w for _, w in pairs)
    r = rnd.uniform(0, total)
    for item, w in pairs:
        if r < w:
            return item
        r -= w
    return pairs[-1][0]


def listen(ts, title, artist, album, secs, track_number):
    return {
        "listened_at": ts,
        "track_metadata": {
            "track_name": title,
            "artist_name": artist,
            "release_name": album,
            "additional_info": {
                "duration_ms": secs * 1000,
                "tracknumber": track_number,
            },
        },
    }


if __name__ == "__main__":
    main()
