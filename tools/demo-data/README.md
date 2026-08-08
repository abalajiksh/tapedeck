# Demo listening history

`tapedeck-demo-history.json` — eight years of synthetic listening, ~35,700
listens, ready to import into an empty Tapedeck so the application has
something to show.

```
Settings → Import → Import from file → tapedeck-demo-history.json
```

Nothing else needs configuring. On the test run it imported in about three
minutes and nothing was rejected or deduplicated.

## What it is

A JSON array of ListenBrainz-shaped listen objects — the shape
`server/import.rs::listen_to_play` reads:

```json
{"listened_at":1533672584,
 "track_metadata":{
   "track_name":"Bohemian Rhapsody",
   "artist_name":"Queen",
   "release_name":"A Night at the Opera",
   "additional_info":{"duration_ms":355000,"tracknumber":7}}}
```

| | |
|---|---|
| Listens | 35,672 |
| Span | 2018-08-07 → 2026-08-07 (2,923 days) |
| Artists / records / distinct tracks | 144 / 169 / 839 |
| Listening hours | 2,648 |
| Sessions after a rebuild | 4,925 |
| Carrying a duration and a track number | all of them |

## Two format decisions

**JSON rather than CSV.** The CSV importer keeps only
`title,artist,album,mbid,time` — it drops the duration and the track number,
and those two are what feed session grouping, listening hours, the per-day
average and an album's running order. A CSV demo history leaves about half the
application blank.

**No MBIDs, deliberately.** A fabricated recording or release MBID would be
written into `mb_cache` and resolve the wrong cover permanently; a fabricated
`caa_id` points at nothing. Every artist, record and track here is real, so
Tapedeck looks each one up the way it would for any genuine listen — which
means the artwork chain, the MusicBrainz backfill and the Every Noise genre
join are themselves part of what gets demonstrated rather than being bypassed.

The consequence is that the history arrives **un-enriched** and fills in over
the following hours as the rate-limited backfills work through it. Ten minutes
after import the test instance had resolved 250 languages and 113 covers, and
Statistics was already showing real genre tags (pop, rock, disco, soul,
electronic) pulled from MusicBrainz.

## What it was built to exercise

The simulation is shaped around what the application actually draws, and each
of these was checked against a running binary rather than assumed:

- **A day has a shape.** Morning, afternoon and evening blocks with a real
  late-night tail, so the hour-of-day heatmap and the Reports clock have
  something in them.
- **Sessions are contiguous**, and consecutive sessions are pushed past the
  30-minute gap `assign_session` breaks on. Rebuilding produced 4,925 sessions
  with a median of 6 tracks, not one session per listen.
- **Records get played as records.** Just under half of all sessions are an
  album run from track one, in order.
- **Taste moves.** Every artist carries an era profile, so the genre-map
  trajectory has a path and the artist charts differ year to year.
- **Discoveries are real discoveries.** Twelve artists have *no* plays at all
  before the month they arrive in. 16 of the 97 monthly Reports name a crush —
  Epitone Project in March 2020, Punch in November 2021, Kendrick Lamar in
  September 2022, Burial in January 2024, Beach House in March 2025.
- **A long tail, not a flat chart.** The top artist has 3,246 plays and the
  bottom of the catalogue has under 100 — roughly the shape of a real 21k-listen
  history. An early version gave every artist about the same count, which is the
  clearest possible tell that a history was generated rather than lived.
- **Things get dropped and things arrive.** Seven artists are heavy in 2019–20
  and effectively gone by 2025, which is the premise of Rediscover.
- **152 tracks played exactly once**, which is the population One-Listen
  Masterpieces selects from.
- **Ambient records only ever go on after midnight**, so tracks actually clear
  the 3 AM Club's bar of 70% of plays between midnight and 5am.

Five of Rediscover's seven collections fill from this file: Forgotten
Favourites, One-Listen Masterpieces, Seasonal Favourites, The 3 AM Club and
Deep Cuts.

## What it cannot cover

A listen import can only carry what a listen import carries. These stay empty
and that is the format's limit, not a gap in the data:

- **Fidelity and quality.** An imported listen has no format, so the Lossless
  and DSD cards degrade to their coverage message and `avg_quality` shows an em
  dash. This is exactly what a real imported history does — measured at 97.4%
  unknown on a genuine 21,390-listen export.
- **Signal chains, gear and devices.** Nothing in the file names one, so chain
  and gear hours are zero. Create a chain and use *History → bulk edit* to
  attribute a stretch of listens to it if you want that half populated.
- **Loves**, so Rediscover's *Loved, Rarely Played* is empty and the Loved page
  has nothing in it.
- **The shelf**, so *Dusty Vinyl* is empty.
- **Skips.** The JSON import path has no skip flag.

## Timezone

Timestamps are laid out against a **UTC** wall clock. Tapedeck buckets hours,
"today" and the trajectory in the *user's* configured zone, so an instance set
to UTC sees the day shape exactly as generated. Any other zone shifts it by that
offset — harmless, but it is why the evening peak may not land where you expect.

The history ends the day before it was generated, so a freshly imported copy
shows nothing under "Today" and an empty deck until something plays.

## Regenerating

```bash
python3 generate.py                          # ~8 years, ends yesterday
python3 generate.py --years 6 --seed 7       # a different, reproducible history
python3 verify.py tapedeck-demo-history.json # audit it before importing
```

No dependencies beyond the standard library. Deterministic: the same seed gives
the same file byte for byte.

`verify.py` is worth running after any change to the catalogue. It reimplements
the three rules that decide whether a listen survives — what `listen_to_play`
requires, what `fuzzy_exists` eats as a duplicate, and the ban on future
timestamps — and then prints the shape of the history, because "it imported"
and "it makes the application look like anything" are different claims.

- `catalog.py` — the records, the artist tiers and the era profiles
- `generate.py` — the simulation
- `verify.py` — the audit
