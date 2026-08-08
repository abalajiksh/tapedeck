"""Catalogue for the Tapedeck demo history.

Real artists, real records, real track titles — because the point of demo data
is that the artwork chain, the MusicBrainz enrichment and the Every Noise genre
join all do *genuine* work on it. Invented names would resolve to nothing and
every one of those features would look broken.

Two things this deliberately is not:

* **Complete.** Each record carries a representative six-to-twelve tracks, not
  its full running order. A partial tracklist is honest; a full one typed from
  memory would be quietly wrong in a dozen places.
* **Identified.** No MBIDs and no `caa_id`s anywhere. A fabricated identifier
  would be written into `mb_cache` and resolve the wrong cover for good, which
  is worse than no identifier at all — with none, Tapedeck looks the record up
  the way it would for any real listen.

Durations are close, not exact. They decide session boundaries, listening hours
and the per-day average, so they need to be plausible; nothing depends on them
being to the second.

`era` is how an artist moves through the eight years — see `generate.py`. It is
the whole reason the genre trajectory has anything to draw.
"""

# artist -> (era profile, language)
#
# Languages are the ISO-ish tags Tapedeck's own language breakdown uses. An
# instrumental record is tagged by the tradition it comes from rather than left
# blank, because "no language" and "no data" are the same cell on that chart.
ARTISTS = {
    # ── the long-running spine of the history ────────────────────────────
    "Cœur de pirate": ("revival", "fra"),
    "Michael Jackson": ("steady", "eng"),
    "Daft Punk": ("steady", "eng"),
    "Simon & Garfunkel": ("steady", "eng"),
    "Queen": ("steady", "eng"),
    "ABBA": ("steady", "eng"),
    "The Beatles": ("steady", "eng"),
    # ── early years, then quieter ────────────────────────────────────────
    "Linkin Park": ("early", "eng"),
    "Evanescence": ("early", "eng"),
    "Nirvana": ("early", "eng"),
    "Europe": ("fade", "eng"),
    "KISS": ("fade", "eng"),
    "Michael Learns to Rock": ("early", "eng"),
    "Britney Spears": ("early", "eng"),
    "Ace of Base": ("fade", "eng"),
    "Roxette": ("fade", "eng"),
    "Red Hot Chili Peppers": ("early", "eng"),
    # ── the middle stretch ───────────────────────────────────────────────
    "Shania Twain": ("mid", "eng"),
    "Diana Krall": ("mid", "eng"),
    "Weyes Blood": ("mid", "eng"),
    "The Marías": ("mid", "eng"),
    "Charlotte Cardin": ("mid", "fra"),
    "Cigarettes After Sex": ("mid", "eng"),
    "Metronomy": ("mid", "eng"),
    "Muse": ("mid", "eng"),
    "Taylor Swift": ("mid", "eng"),
    "twenty one pilots": ("mid", "eng"),
    "The Weeknd": ("mid", "eng"),
    "Halsey": ("mid", "eng"),
    "OneRepublic": ("mid", "eng"),
    "Sarah McLachlan": ("mid", "eng"),
    "Chris Isaak": ("mid", "eng"),
    "Madonna": ("mid", "eng"),
    "Whitney Houston": ("mid", "eng"),
    "Elvis Presley": ("mid", "eng"),
    "George Michael": ("mid", "eng"),
    "Enrique Iglesias": ("mid", "spa"),
    # ── arrived late ─────────────────────────────────────────────────────
    "Larkin Poe": ("late", "eng"),
    "The Beaches": ("late", "eng"),
    "Becky G": ("late", "spa"),
    "Khruangbin": ("late", "eng"),
    "Fontaines D.C.": ("late", "eng"),
    "Little Simz": ("late", "eng"),
    "Hania Rani": ("late", "pol"),
    "Sault": ("late", "eng"),
    # ── the crushes: discovered in one month and played into the ground ──
    "Epitone Project": ("crush:2020-03", "kor"),
    "Punch": ("crush:2021-11", "kor"),
    "Kim Na Young": ("crush:2022-06", "kor"),
    "Mariya Takeuchi": ("crush:2019-07", "jpn"),
    "Tatsuro Yamashita": ("crush:2023-02", "jpn"),
    "Sigur Rós": ("crush:2019-02", "isl"),
    "Burial": ("crush:2024-01", "eng"),
    "Kendrick Lamar": ("crush:2022-09", "eng"),
    "Fela Kuti": ("crush:2023-08", "yor"),
    "Aphex Twin": ("crush:2024-10", "eng"),
    "Beach House": ("crush:2025-03", "eng"),
    "Bad Bunny": ("crush:2025-09", "spa"),
    # ── Tamil and Hindi film music, present throughout ───────────────────
    "A. R. Rahman": ("steady", "tam"),
    "Ilaiyaraaja": ("wave", "tam"),
    "Harris Jayaraj": ("wave", "tam"),
    "Yuvan Shankar Raja": ("mid", "tam"),
    "Anirudh Ravichander": ("late", "tam"),
    "D. Imman": ("mid", "tam"),
    "Shankar–Ehsaan–Loy": ("mid", "hin"),
    "Amit Trivedi": ("late", "hin"),
    # ── the quiet, always-there records ──────────────────────────────────
    "Johann Sebastian Bach": ("steady", "deu"),
    "Frédéric Chopin": ("steady", "pol"),
    "Claude Debussy": ("wave", "fra"),
    "Erik Satie": ("wave", "fra"),
    "Miles Davis": ("steady", "eng"),
    "Bill Evans": ("wave", "eng"),
    "João Gilberto": ("wave", "por"),
    "Buena Vista Social Club": ("wave", "spa"),
    "Brian Eno": ("steady", "eng"),
    "Boards of Canada": ("wave", "eng"),
    "Nina Simone": ("steady", "eng"),
    "A Tribe Called Quest": ("wave", "eng"),
    "Talk Talk": ("revival", "eng"),
    "Massive Attack": ("revival", "eng"),
    # ── December only ────────────────────────────────────────────────────
    "Nat King Cole": ("xmas", "eng"),
    "Vince Guaraldi Trio": ("xmas", "eng"),
    "The Ronettes": ("xmas", "eng"),
}

# How much of the history each artist gets, before the era profile shapes *when*.
#
# This exists because the first version of this generator did without it, and
# produced a top-12 running from 586 plays down to 520 — every artist as big as
# every other. A real history is nothing like that: the top of the 21,390-listen
# history this was modelled on runs 2870, 2522, 399, 362, 332, and the long tail
# is most of the catalogue. A flat chart is the single clearest tell that a
# history was generated rather than lived, and it makes Top Charts, the Reports
# crush and the artist pages all read as noise.
#
# Anything not listed is `regular`. The multipliers are chosen so a `core`
# artist lands around 7% of all listening and an `occasional` one around 0.3%.
TIERS = {
    "core": ["Cœur de pirate", "Michael Jackson"],
    "major": [
        "Daft Punk", "A. R. Rahman", "Shania Twain", "Linkin Park", "Harris Jayaraj",
        "Weyes Blood", "Diana Krall", "Ilaiyaraaja", "Queen", "Simon & Garfunkel",
        "ABBA", "Britney Spears", "Michael Learns to Rock", "The Marías", "Europe",
        # The crushes are majors too: a crush that peaks below the regulars is
        # not a crush, and the Reports card exists to find exactly this spike.
        "Epitone Project", "Punch", "Kim Na Young", "Mariya Takeuchi",
        "Tatsuro Yamashita", "Sigur Rós", "Burial", "Kendrick Lamar", "Fela Kuti",
        "Aphex Twin", "Beach House", "Bad Bunny",
    ],
    "occasional": [
        "Erik Satie", "Claude Debussy", "Bill Evans", "João Gilberto",
        "Buena Vista Social Club", "Brian Eno", "Boards of Canada", "Talk Talk",
        "A Tribe Called Quest", "Hania Rani", "Sault", "Amit Trivedi", "D. Imman",
        "Elvis Presley", "Whitney Houston", "George Michael", "Madonna",
        # The three Christmas records are deliberately *not* here. They only
        # exist for one month a year, so an `occasional` multiplier on top of
        # that left them at 11–38 plays across eight Decembers — present in the
        # data and invisible on every screen. December has to actually look
        # like December.
    ],
}

TIER_WEIGHT = {"core": 8.0, "major": 3.0, "regular": 1.0, "occasional": 0.35}


def tier_of(artist):
    for name, members in TIERS.items():
        if artist in members:
            return name
    return "regular"


# (artist, album, tracks[(title, seconds)])
#
# Order is the running order; the generator turns the index into `tracknumber`,
# which is what makes an album page read as a record rather than a bag of songs.
ALBUMS = [
    # ── Cœur de pirate ───────────────────────────────────────────────────
    ("Cœur de pirate", "Cœur de pirate", [
        ("Ensemble", 168), ("Comme des enfants", 202), ("Le long du large", 175),
        ("Fondu au noir", 191), ("Printemps", 158), ("Berceuse", 183),
        ("Pour un infidèle", 224), ("939", 165), ("Intermission", 121),
    ]),
    ("Cœur de pirate", "Blonde", [
        ("Lève tes barricades", 194), ("Adieu", 195), ("Golden Baby", 208),
        ("Danse et danse", 187), ("Ava", 213), ("Loin d'ici", 178),
        ("Place de la République", 226), ("Verseau", 199), ("Cap Diamant", 205),
    ]),
    ("Cœur de pirate", "Roses", [
        ("Oublie-moi", 211), ("Carry On", 198), ("Crier tout bas", 224),
        ("Drapeau blanc", 187), ("Undone", 203), ("Ne m'appelle pas", 216),
        ("Alexandre", 195), ("Someone Special", 189),
    ]),
    ("Cœur de pirate", "Impossible à aimer", [
        ("On s'aimera toujours", 207), ("Impossible à aimer", 219),
        ("T'es belle", 198), ("Trahison", 184), ("Ne m'oublie pas", 213),
        ("Combien de temps", 226),
    ]),
    # ── Michael Jackson ──────────────────────────────────────────────────
    ("Michael Jackson", "Thriller", [
        ("Wanna Be Startin' Somethin'", 363), ("Baby Be Mine", 260),
        ("The Girl Is Mine", 222), ("Thriller", 357), ("Beat It", 258),
        ("Billie Jean", 294), ("Human Nature", 246),
        ("P.Y.T. (Pretty Young Thing)", 239), ("The Lady in My Life", 299),
    ]),
    ("Michael Jackson", "Bad", [
        ("Bad", 247), ("The Way You Make Me Feel", 297), ("Speed Demon", 240),
        ("Liberian Girl", 234), ("Another Part of Me", 234),
        ("Man in the Mirror", 319), ("I Just Can't Stop Loving You", 253),
        ("Dirty Diana", 289), ("Smooth Criminal", 257),
    ]),
    ("Michael Jackson", "Off the Wall", [
        ("Don't Stop 'Til You Get Enough", 366), ("Rock with You", 220),
        ("Working Day and Night", 314), ("Off the Wall", 248),
        ("She's Out of My Life", 218), ("Burn This Disco Out", 219),
    ]),
    ("Michael Jackson", "Dangerous", [
        ("Jam", 338), ("Remember the Time", 240), ("In the Closet", 296),
        ("Heal the World", 383), ("Black or White", 254),
        ("Who Is It", 400), ("Will You Be There", 351),
    ]),
    # ── Daft Punk ────────────────────────────────────────────────────────
    ("Daft Punk", "Discovery", [
        ("One More Time", 320), ("Aerodynamic", 212), ("Digital Love", 301),
        ("Harder, Better, Faster, Stronger", 224), ("Crescendolls", 211),
        ("Nightvision", 104), ("Superheroes", 237), ("Veridis Quo", 345),
        ("Face to Face", 240), ("Too Long", 600),
    ]),
    ("Daft Punk", "Random Access Memories", [
        ("Give Life Back to Music", 274), ("The Game of Love", 322),
        ("Giorgio by Moroder", 544), ("Within", 228), ("Instant Crush", 337),
        ("Lose Yourself to Dance", 353), ("Touch", 498),
        ("Get Lucky", 369), ("Doin' It Right", 251), ("Contact", 383),
    ]),
    ("Daft Punk", "Homework", [
        ("Da Funk", 328), ("Around the World", 429), ("Revolution 909", 335),
        ("Phœnix", 297), ("Rollin' & Scratchin'", 447), ("Alive", 302),
    ]),
    # ── the steady classics ──────────────────────────────────────────────
    ("Simon & Garfunkel", "Bridge over Troubled Water", [
        ("Bridge over Troubled Water", 295), ("El Cóndor Pasa (If I Could)", 187),
        ("Cecilia", 175), ("Keep the Customer Satisfied", 154),
        ("So Long, Frank Lloyd Wright", 202), ("The Boxer", 308),
        ("The Only Living Boy in New York", 240), ("Song for the Asking", 100),
    ]),
    ("Simon & Garfunkel", "Sounds of Silence", [
        ("The Sound of Silence", 185), ("Leaves That Are Green", 145),
        ("Homeward Bound", 150), ("Richard Cory", 178),
        ("I Am a Rock", 172), ("Kathy's Song", 202),
    ]),
    ("Queen", "A Night at the Opera", [
        ("Death on Two Legs", 223), ("Lazing on a Sunday Afternoon", 68),
        ("I'm in Love with My Car", 185), ("You're My Best Friend", 172),
        ("'39", 211), ("Love of My Life", 219),
        ("Bohemian Rhapsody", 355), ("God Save the Queen", 74),
    ]),
    ("Queen", "News of the World", [
        ("We Will Rock You", 122), ("We Are the Champions", 179),
        ("Sheer Heart Attack", 206), ("All Dead, All Dead", 190),
        ("Spread Your Wings", 274), ("It's Late", 386),
    ]),
    ("ABBA", "Arrival", [
        ("When I Kissed the Teacher", 179), ("Dancing Queen", 231),
        ("My Love, My Life", 232), ("Dum Dum Diddle", 175),
        ("Knowing Me, Knowing You", 242), ("Money, Money, Money", 185),
        ("Why Did It Have to Be Me?", 203),
    ]),
    ("ABBA", "The Album", [
        ("Eagle", 351), ("Take a Chance on Me", 244), ("One Man, One Woman", 274),
        ("The Name of the Game", 291), ("Move On", 285), ("Thank You for the Music", 231),
    ]),
    ("The Beatles", "Abbey Road", [
        ("Come Together", 259), ("Something", 182), ("Oh! Darling", 206),
        ("Octopus's Garden", 171), ("Here Comes the Sun", 185),
        ("Because", 165), ("You Never Give Me Your Money", 242),
        ("Golden Slumbers", 91), ("Carry That Weight", 96), ("The End", 140),
    ]),
    ("The Beatles", "Revolver", [
        ("Taxman", 156), ("Eleanor Rigby", 128), ("Here, There and Everywhere", 145),
        ("Yellow Submarine", 160), ("Good Day Sunshine", 129),
        ("For No One", 121), ("Got to Get You into My Life", 145),
        ("Tomorrow Never Knows", 178),
    ]),
    # ── the early-years block ────────────────────────────────────────────
    ("Linkin Park", "Hybrid Theory", [
        ("Papercut", 184), ("One Step Closer", 155), ("With You", 203),
        ("Points of Authority", 200), ("Crawling", 208), ("Runaway", 184),
        ("In the End", 216), ("A Place for My Head", 184), ("Pushing Me Away", 191),
    ]),
    ("Linkin Park", "Meteora", [
        ("Don't Stay", 187), ("Somewhere I Belong", 213), ("Lying from You", 175),
        ("Faint", 162), ("Figure.09", 197), ("Breaking the Habit", 196),
        ("From the Inside", 175), ("Numb", 187),
    ]),
    ("Evanescence", "Fallen", [
        ("Going Under", 215), ("Bring Me to Life", 237), ("Everybody's Fool", 195),
        ("My Immortal", 264), ("Haunted", 187), ("Tourniquet", 249),
        ("Imaginary", 197), ("Hello", 217),
    ]),
    ("Nirvana", "Nevermind", [
        ("Smells Like Teen Spirit", 301), ("In Bloom", 254), ("Come as You Are", 219),
        ("Breed", 183), ("Lithium", 257), ("Polly", 177),
        ("Drain You", 224), ("Something in the Way", 232),
    ]),
    ("Nirvana", "MTV Unplugged in New York", [
        ("About a Girl", 217), ("Come as You Are", 250), ("The Man Who Sold the World", 260),
        ("Pennyroyal Tea", 220), ("Where Did You Sleep Last Night", 322),
    ]),
    ("Europe", "The Final Countdown", [
        ("The Final Countdown", 310), ("Rock the Night", 244),
        ("Carrie", 267), ("Danger on the Track", 245),
        ("Cherokee", 267), ("Time Has Come", 292),
    ]),
    ("KISS", "Destroyer", [
        ("Detroit Rock City", 328), ("King of the Night Time World", 210),
        ("God of Thunder", 254), ("Great Expectations", 265),
        ("Shout It Out Loud", 220), ("Beth", 165), ("Do You Love Me", 213),
    ]),
    ("Michael Learns to Rock", "Played on Pepper", [
        ("Sleeping Child", 254), ("25 Minutes", 265), ("That's Why (You Go Away)", 254),
        ("Someday", 233), ("Out of the Blue", 226), ("Nothing to Lose", 241),
    ]),
    ("Britney Spears", "...Baby One More Time", [
        ("...Baby One More Time", 211), ("(You Drive Me) Crazy", 197),
        ("Sometimes", 259), ("Soda Pop", 199), ("Born to Make You Happy", 214),
        ("From the Bottom of My Broken Heart", 313),
    ]),
    ("Britney Spears", "Oops!... I Did It Again", [
        ("Oops!... I Did It Again", 211), ("Stronger", 203),
        ("Don't Go Knockin' on My Door", 202), ("Lucky", 205),
        ("What U See (Is What U Get)", 224), ("Dear Diary", 129),
    ]),
    ("Ace of Base", "The Sign", [
        ("All That She Wants", 210), ("The Sign", 191), ("Don't Turn Around", 234),
        ("Living in Danger", 224), ("Happy Nation", 246), ("Wheel of Fortune", 232),
    ]),
    ("Roxette", "Look Sharp!", [
        ("The Look", 209), ("Dressed for Success", 265), ("Sleeping Single", 217),
        ("Listen to Your Heart", 328), ("Dangerous", 205), ("Cry", 234),
    ]),
    ("Red Hot Chili Peppers", "Californication", [
        ("Around the World", 238), ("Scar Tissue", 217), ("Otherside", 255),
        ("Californication", 329), ("Easily", 231), ("Road Trippin'", 205),
    ]),
    # ── the middle stretch ───────────────────────────────────────────────
    ("Shania Twain", "Come On Over", [
        ("Man! I Feel Like a Woman!", 233), ("I'm Holdin' On to Love", 202),
        ("Love Gets Me Every Time", 194), ("Don't Be Stupid", 213),
        ("From This Moment On", 291), ("Come On Over", 175),
        ("You're Still the One", 213), ("That Don't Impress Me Much", 218),
    ]),
    ("Shania Twain", "Up!", [
        ("Up!", 173), ("She's Not Just a Pretty Face", 217), ("Ka-Ching!", 202),
        ("Forever and for Always", 265), ("I'm Gonna Getcha Good!", 273),
        ("Juanita", 208), ("Thank You Baby!", 213),
    ]),
    ("Diana Krall", "The Look of Love", [
        ("S'Wonderful", 269), ("Love Letters", 292), ("I Remember You", 258),
        ("Cry Me a River", 289), ("Besame Mucho", 396), ("The Look of Love", 290),
    ]),
    ("Diana Krall", "Live in Paris", [
        ("I Love Being Here with You", 279), ("Let's Fall in Love", 320),
        ("'Deed I Do", 265), ("The Look of Love", 344),
        ("East of the Sun", 380), ("A Case of You", 351),
    ]),
    ("Weyes Blood", "Titanic Rising", [
        ("A Lot's Gonna Change", 268), ("Andromeda", 300), ("Everyday", 310),
        ("Something to Believe", 285), ("Movies", 349),
        ("Wild Time", 366), ("Picture Me Better", 254),
    ]),
    ("Weyes Blood", "And in the Darkness, Hearts Aglow", [
        ("It's Not Just Me, It's Everybody", 384), ("Children of the Empire", 300),
        ("Grapevine", 348), ("God Turn Me Into a Flower", 380),
        ("Twin Flame", 296), ("A Given Thing", 285),
    ]),
    ("The Marías", "CINEMA", [
        ("Just a Feeling", 208), ("Calling U Back", 195), ("Hush", 175),
        ("All I Really Want Is You", 220), ("Little by Little", 202),
        ("Un Millón", 188), ("Talk to Me", 224),
    ]),
    ("The Marías", "Submarine", [
        ("Ay No Puedo", 199), ("Run Your Mouth", 214), ("Lejos de Ti", 186),
        ("No One Noticed", 267), ("Hamptons", 198), ("Sienna", 205),
    ]),
    ("Charlotte Cardin", "Phoenix", [
        ("Phoenix", 214), ("Passive Aggressive", 197), ("Meaningless", 208),
        ("Sex to Me", 224), ("Daddy", 231), ("Anyone Who Loves Me", 202),
    ]),
    ("Cigarettes After Sex", "Cigarettes After Sex", [
        ("K.", 297), ("Each Time You Fall in Love", 268), ("Sunsetz", 260),
        ("Apocalypse", 291), ("Flash", 220), ("Sweet", 279), ("Young & Dumb", 246),
    ]),
    ("Metronomy", "The English Riviera", [
        ("The Look", 268), ("She Wants", 254), ("The Bay", 285),
        ("Everything Goes My Way", 250), ("Corinne", 191), ("Some Written", 271),
    ]),
    ("Muse", "Black Holes and Revelations", [
        ("Take a Bow", 275), ("Starlight", 240), ("Supermassive Black Hole", 212),
        ("Map of the Problematique", 258), ("Invincible", 300),
        ("Knights of Cydonia", 366),
    ]),
    ("Taylor Swift", "1989", [
        ("Welcome to New York", 212), ("Blank Space", 231), ("Style", 231),
        ("Out of the Woods", 235), ("Shake It Off", 219),
        ("Wildest Dreams", 220), ("This Love", 250),
    ]),
    ("Taylor Swift", "folklore", [
        ("the 1", 210), ("cardigan", 239), ("the last great american dynasty", 231),
        ("exile", 285), ("my tears ricochet", 255), ("august", 261),
        ("betty", 294), ("peace", 234),
    ]),
    ("twenty one pilots", "Blurryface", [
        ("Heavydirtysoul", 234), ("Stressed Out", 202), ("Ride", 214),
        ("Fairly Local", 207), ("Tear in My Heart", 188),
        ("Lane Boy", 260), ("Message Man", 240),
    ]),
    ("The Weeknd", "After Hours", [
        ("Alone Again", 250), ("Hardest to Love", 211), ("Scared to Live", 191),
        ("Heartless", 198), ("Faith", 283), ("Blinding Lights", 200),
        ("In Your Eyes", 237), ("Save Your Tears", 215),
    ]),
    ("Halsey", "Badlands", [
        ("Castle", 245), ("Hold Me Down", 219), ("New Americana", 213),
        ("Drive", 213), ("Colors", 233), ("Gasoline", 193), ("Control", 216),
    ]),
    ("OneRepublic", "Waking Up", [
        ("Made for You", 226), ("All the Right Moves", 239), ("Secrets", 225),
        ("Marchin On", 233), ("Good Life", 254), ("All This Time", 240),
    ]),
    ("Sarah McLachlan", "Surfacing", [
        ("Building a Mystery", 249), ("I Love You", 268), ("Sweet Surrender", 220),
        ("Adia", 249), ("Do What You Have to Do", 213), ("Angel", 270),
    ]),
    ("Chris Isaak", "Heart Shaped World", [
        ("Heart Shaped World", 208), ("Kings of the Highway", 227),
        ("Wicked Game", 288), ("Blue Spanish Sky", 224),
        ("Wrong to Love You", 197), ("Diddley Daddy", 178),
    ]),
    ("Madonna", "Ray of Light", [
        ("Drowned World/Substitute for Love", 305), ("Swim", 289),
        ("Ray of Light", 321), ("Candy Perfume Girl", 274),
        ("Frozen", 372), ("The Power of Good-Bye", 291),
    ]),
    ("Whitney Houston", "Whitney Houston", [
        ("You Give Good Love", 271), ("Saving All My Love for You", 236),
        ("Greatest Love of All", 274), ("How Will I Know", 229),
        ("All at Once", 268), ("Thinking About You", 322),
    ]),
    ("Elvis Presley", "Elvis' Golden Records", [
        ("Hound Dog", 136), ("Loving You", 140), ("All Shook Up", 118),
        ("Heartbreak Hotel", 128), ("Jailhouse Rock", 146),
        ("Love Me Tender", 168), ("Don't Be Cruel", 122),
    ]),
    ("George Michael", "Faith", [
        ("Faith", 193), ("Father Figure", 337), ("I Want Your Sex", 393),
        ("One More Try", 349), ("Hard Day", 288), ("Kissing a Fool", 275),
    ]),
    ("Enrique Iglesias", "Escape", [
        ("Escape", 202), ("Don't Turn Off the Lights", 224), ("Love to See You Cry", 218),
        ("Hero", 202), ("She Be the One", 231), ("Maybe", 254),
    ]),
    # ── arrived late ─────────────────────────────────────────────────────
    ("Larkin Poe", "Self Made Man", [
        ("She's a Self Made Man", 197), ("Holy Ghost Fire", 208),
        ("Keep Diggin'", 190), ("Back Down South", 224),
        ("Tears of Blue to Gold", 231), ("Every Bird That Flies", 245),
    ]),
    ("The Beaches", "Blame My Ex", [
        ("Blame Brett", 191), ("Everything Is Boring", 178),
        ("Shower Beer", 203), ("Kismet", 195),
        ("What Do You Want?", 187), ("Me & Me", 210),
    ]),
    ("Becky G", "ESQUEMAS", [
        ("BUEN DÍA", 175), ("MAMIII", 226), ("FULANITO", 172),
        ("BAILÉ CON MI EX", 168), ("DOLLAR", 184), ("KILL BILL", 179),
    ]),
    ("Khruangbin", "Con Todo El Mundo", [
        ("Cómo Me Quieres", 258), ("Lady and Man", 213), ("Maria También", 231),
        ("August 10", 227), ("Evan Finds the Third Room", 264),
        ("Friday Morning", 268), ("A Hymn", 219),
    ]),
    ("Fontaines D.C.", "Skinty Fia", [
        ("In ár gCroíthe go deo", 366), ("Big Shot", 218), ("How Cold Love Is", 245),
        ("Jackie Down the Line", 231), ("Bloomsday", 253), ("Roman Holiday", 254),
    ]),
    ("Little Simz", "Sometimes I Might Be Introvert", [
        ("Introvert", 293), ("Woman", 217), ("Two Worlds Apart", 200),
        ("I Love You, I Hate You", 265), ("Little Q, Pt. 2", 258),
        ("Point and Kill", 214), ("Miss Understood", 253),
    ]),
    ("Hania Rani", "Esja", [
        ("Eden", 264), ("Glass", 300), ("Sun", 245),
        ("Now, Run", 288), ("Esja", 331), ("Hawaii Oslo", 274),
    ]),
    ("Sault", "Untitled (Rise)", [
        ("Strong", 195), ("Fearless", 213), ("Rise", 224),
        ("I Just Want to Dance", 240), ("Free", 209), ("Little Boy", 258),
    ]),
    # ── the crushes ──────────────────────────────────────────────────────
    ("Epitone Project", "Sunny Day, Cloudy Day", [
        ("잘 지내나요", 265), ("긴 여행의 시작", 288), ("이제 그만 사랑하겠습니다", 274),
        ("우리 함께", 251), ("초승달", 240), ("첫사랑", 292),
    ]),
    ("Punch", "Snowy", [
        ("Snowy", 224), ("Sometimes", 213), ("Say Yes", 231),
        ("Stay With Me", 219), ("Everytime", 227), ("Done for Me", 205),
    ]),
    ("Kim Na Young", "Sunset", [
        ("사랑이 지나가면", 258), ("그때가 좋았어", 244), ("Sunset", 231),
        ("널 사랑하는 일", 267), ("이별하는 중입니다", 253),
    ]),
    ("Mariya Takeuchi", "Variety", [
        ("Plastic Love", 292), ("Mou Ichido", 258), ("Kanashimi no Jody", 271),
        ("Honki de Only You", 245), ("Shinkokyū Shite", 263),
    ]),
    ("Tatsuro Yamashita", "For You", [
        ("Sparkle", 300), ("Music Book", 285), ("Morning Glory", 291),
        ("Futari", 264), ("Loveland, Island", 279), ("Your Eyes", 322),
    ]),
    ("Sigur Rós", "Ágætis byrjun", [
        ("Svefn-g-englar", 601), ("Starálfur", 415), ("Flugufrelsarinn", 460),
        ("Ný batterí", 508), ("Viðrar vel til loftárása", 622),
        ("Ágætis byrjun", 448),
    ]),
    ("Burial", "Untrue", [
        ("Archangel", 236), ("Near Dark", 236), ("Ghost Hardware", 300),
        ("Etched Headplate", 320), ("In McDonalds", 129),
        ("Untrue", 227), ("Shell of Light", 281), ("Raver", 288),
    ]),
    ("Kendrick Lamar", "good kid, m.A.A.d city", [
        ("Sherane a.k.a Master Splinter's Daughter", 273),
        ("Bitch, Don't Kill My Vibe", 310), ("Backseat Freestyle", 212),
        ("Money Trees", 386), ("Poetic Justice", 300),
        ("m.A.A.d city", 350), ("Sing About Me", 723),
    ]),
    ("Fela Kuti", "Zombie", [
        ("Zombie", 745), ("Mister Follow Follow", 776),
        ("Observation Is No Crime", 700), ("Mistake", 800),
    ]),
    ("Aphex Twin", "Selected Ambient Works 85-92", [
        ("Xtal", 293), ("Tha", 543), ("Pulsewidth", 234),
        ("Ageispolis", 344), ("i", 68), ("Green Calx", 366),
        ("Heliosphan", 292), ("Schottkey 7th Path", 306),
    ]),
    ("Beach House", "Teen Dream", [
        ("Zebra", 296), ("Silver Soul", 296), ("Norway", 200),
        ("Walk in the Park", 336), ("Used to Be", 258),
        ("10 Mile Stereo", 320), ("Take Care", 348),
    ]),
    ("Bad Bunny", "Un Verano Sin Ti", [
        ("Moscow Mule", 245), ("Después de la Playa", 230), ("Me Porto Bonito", 178),
        ("Tití Me Preguntó", 243), ("Ojitos Lindos", 258),
        ("Efecto", 194), ("Party", 232),
    ]),
    # ── Tamil and Hindi film music ───────────────────────────────────────
    ("A. R. Rahman", "Roja", [
        ("Chinna Chinna Aasai", 313), ("Kadhal Rojave", 340),
        ("Pudhiya Mugam", 298), ("Rukkumani Rukkumani", 322),
        ("Puthu Vellai Mazhai", 351),
    ]),
    ("A. R. Rahman", "Bombay", [
        ("Uyire Uyire", 340), ("Kannalane", 351), ("Halo Halo", 288),
        ("Kuchi Kuchi Rakkamma", 306), ("Bombay Theme", 265),
    ]),
    ("A. R. Rahman", "Alaipayuthey", [
        ("Snehidhane", 366), ("Pachai Nirame", 344), ("Evano Oruvan", 310),
        ("September Madham", 331), ("Endrendrum Punnagai", 322),
    ]),
    ("Ilaiyaraaja", "Mouna Ragam", [
        ("Chinna Chinna Vanna Kuyil", 288), ("Nilave Vaa", 320),
        ("Mandram Vandha", 331), ("Oh Ho Ho Ho", 274),
    ]),
    ("Ilaiyaraaja", "Thalapathi", [
        ("Rakkamma Kaiya Thattu", 351), ("Sundari Kannal Oru Sethi", 366),
        ("Kaatukuyilu", 322), ("Yamunai Aatrile", 340),
    ]),
    ("Harris Jayaraj", "Minnale", [
        ("Vaseegara", 340), ("Oh Mama Mama", 288), ("Azhagiya Theeye", 322),
        ("Iru Vizhi Unadhu", 331), ("Nenjodu Kalanthidu", 306),
    ]),
    ("Harris Jayaraj", "Ghajini", [
        ("Suttum Vizhi", 331), ("Oru Maalai", 322), ("Rahatulla", 288),
        ("Iragai Pole", 306), ("Uyirin Uyire", 340),
    ]),
    ("Yuvan Shankar Raja", "Kaadhal Kondein", [
        ("Devathaiyai Kanden", 331), ("Kanave Kalaiyathe", 322),
        ("En Nenjil", 288), ("Nenjodu Kalanthidu", 306),
    ]),
    ("Anirudh Ravichander", "3", [
        ("Why This Kolaveri Di", 250), ("Idhazhin Oram", 331),
        ("Come On Girls", 274), ("Po Nee Po", 322), ("Kannazhaga", 288),
    ]),
    ("D. Imman", "Kumki", [
        ("Sollitaley Ava Kaadhala", 322), ("Onnum Puriyala", 288),
        ("Ayayayo", 306), ("Aanandha Yazhai", 331),
    ]),
    ("Shankar–Ehsaan–Loy", "Dil Chahta Hai", [
        ("Dil Chahta Hai", 340), ("Jaane Kyon", 288), ("Woh Ladki Hai Kahan", 322),
        ("Tanhayee", 331), ("Kaisi Hai Ye Rut", 274),
    ]),
    ("Amit Trivedi", "Dev.D", [
        ("Emosanal Attyachar", 240), ("Pardesi", 288), ("Nayan Tarse", 253),
        ("Dhol Yaara Dhol", 274), ("Duniya", 231),
    ]),
    # ── the quiet records ────────────────────────────────────────────────
    ("Johann Sebastian Bach", "Goldberg Variations", [
        ("Aria", 232), ("Variatio 1", 108), ("Variatio 5", 96),
        ("Variatio 13", 292), ("Variatio 25", 402),
        ("Variatio 30 Quodlibet", 118), ("Aria da capo", 220),
    ]),
    ("Johann Sebastian Bach", "Cello Suites", [
        ("Suite No. 1: Prélude", 148), ("Suite No. 1: Allemande", 262),
        ("Suite No. 1: Courante", 155), ("Suite No. 1: Sarabande", 172),
        ("Suite No. 3: Prélude", 220), ("Suite No. 3: Bourrée", 199),
    ]),
    ("Frédéric Chopin", "Nocturnes", [
        ("Nocturne in B-flat minor, Op. 9 No. 1", 320),
        ("Nocturne in E-flat major, Op. 9 No. 2", 271),
        ("Nocturne in F-sharp major, Op. 15 No. 2", 218),
        ("Nocturne in C-sharp minor, Op. 27 No. 1", 316),
        ("Nocturne in D-flat major, Op. 27 No. 2", 358),
    ]),
    ("Claude Debussy", "Préludes", [
        ("La fille aux cheveux de lin", 148), ("La cathédrale engloutie", 383),
        ("Des pas sur la neige", 256), ("Voiles", 219),
        ("Bruyères", 168), ("Feux d'artifice", 271),
    ]),
    ("Erik Satie", "Gymnopédies & Gnossiennes", [
        ("Gymnopédie No. 1", 210), ("Gymnopédie No. 2", 175),
        ("Gymnopédie No. 3", 168), ("Gnossienne No. 1", 224),
        ("Gnossienne No. 3", 165),
    ]),
    ("Miles Davis", "Kind of Blue", [
        ("So What", 562), ("Freddie Freeloader", 586), ("Blue in Green", 337),
        ("All Blues", 691), ("Flamenco Sketches", 566),
    ]),
    ("Bill Evans", "Waltz for Debby", [
        ("My Foolish Heart", 258), ("Waltz for Debby", 405),
        ("Detour Ahead", 442), ("My Romance", 442), ("Some Other Time", 300),
    ]),
    ("João Gilberto", "Getz/Gilberto", [
        ("The Girl from Ipanema", 325), ("Doralice", 164), ("Pra Machucar Meu Coração", 215),
        ("Desafinado", 255), ("Corcovado", 250), ("O Grande Amor", 205),
    ]),
    ("Buena Vista Social Club", "Buena Vista Social Club", [
        ("Chan Chan", 256), ("De Camino a La Vereda", 305),
        ("El Cuarto de Tula", 434), ("Dos Gardenias", 179),
        ("Veinte Años", 220), ("Candela", 302),
    ]),
    ("Brian Eno", "Ambient 1: Music for Airports", [
        ("1/1", 1027), ("2/1", 490), ("1/2", 705), ("2/2", 363),
    ]),
    ("Boards of Canada", "Music Has the Right to Children", [
        ("Wildlife Analysis", 96), ("An Eagle in Your Mind", 386),
        ("Telephasic Workshop", 391), ("Roygbiv", 151),
        ("Aquarius", 341), ("Olson", 91),
    ]),
    ("Nina Simone", "I Put a Spell on You", [
        ("I Put a Spell on You", 156), ("Tomorrow Is My Turn", 195),
        ("Ne Me Quitte Pas", 214), ("Feeling Good", 175),
        ("Sinnerman", 604), ("Beautiful Land", 141),
    ]),
    ("A Tribe Called Quest", "The Low End Theory", [
        ("Excursions", 232), ("Buggin' Out", 218), ("Jazz (We've Got)", 249),
        ("Check the Rhime", 216), ("Scenario", 250), ("Verses from the Abstract", 240),
    ]),
    ("Talk Talk", "Spirit of Eden", [
        ("The Rainbow", 350), ("Eden", 386), ("Desire", 427),
        ("Inheritance", 337), ("I Believe in You", 500), ("Wealth", 366),
    ]),
    ("Massive Attack", "Mezzanine", [
        ("Angel", 380), ("Risingson", 297), ("Teardrop", 330),
        ("Inertia Creeps", 355), ("Dissolved Girl", 366), ("Group Four", 495),
    ]),
    # ── December only ────────────────────────────────────────────────────
    ("Nat King Cole", "The Christmas Song", [
        ("The Christmas Song", 189), ("Deck the Halls", 128),
        ("O Tannenbaum", 143), ("Caroling, Caroling", 130),
        ("The First Noel", 168), ("Silent Night", 189),
    ]),
    ("Vince Guaraldi Trio", "A Charlie Brown Christmas", [
        ("O Tannenbaum", 315), ("What Child Is This", 296),
        ("Linus and Lucy", 183), ("Christmas Time Is Here", 316),
        ("Skating", 165), ("My Little Drum", 190),
    ]),
    ("The Ronettes", "A Christmas Gift for You", [
        ("Sleigh Ride", 182), ("Frosty the Snowman", 156),
        ("I Saw Mommy Kissing Santa Claus", 168),
    ]),
]

# Records that only ever go on after midnight.
#
# The 3 AM Club asks for tracks played *at least three times* with **70% or more
# of those plays between midnight and 5am** (`db::late_night`). Nothing meets
# that if late-night sessions draw from the same pool as every other session —
# a track played 40 times in daylight and 4 times at 2am is not a 3 AM track,
# and the first version of this generator left that collection empty.
#
# So these are drawn from *only* in the small hours. Which is also just true:
# nobody puts Music for Airports on at half past nine in the morning.
NOCTURNAL = {"Brian Eno", "Erik Satie", "Hania Rani", "Boards of Canada"}

# The tail: things tried once and never returned to.
#
# `db::heard_once` wants tracks with exactly one play. A simulation that only
# ever draws from a rotation produces none at all — the first version had 678
# unique tracks across 35,000 listens, which is not what a real history looks
# like, and it left One-Listen Masterpieces empty too.
#
# These are only ever reached by an "exploration" pick, so most of them land on
# one play and stay there. They double as breadth for the genre map, which is
# the other thing a small catalogue starves.
EXPLORE = [
    ("Portishead", "Dummy", [("Glory Box", 305), ("Roads", 302), ("Sour Times", 254), ("Wandering Star", 216)]),
    ("Radiohead", "In Rainbows", [("15 Step", 237), ("Nude", 255), ("Weird Fishes", 321), ("Reckoner", 290)]),
    ("Björk", "Homogenic", [("Jóga", 305), ("Bachelorette", 315), ("All Is Full of Love", 268), ("Unravel", 249)]),
    ("Fleetwood Mac", "Rumours", [("Dreams", 257), ("Go Your Own Way", 223), ("The Chain", 270), ("Songbird", 200)]),
    ("Stevie Wonder", "Innervisions", [("Living for the City", 461), ("Higher Ground", 222), ("Golden Lady", 298)]),
    ("Marvin Gaye", "What's Going On", [("What's Going On", 233), ("Mercy Mercy Me", 193), ("Inner City Blues", 316)]),
    ("Bob Dylan", "Blood on the Tracks", [("Tangled Up in Blue", 341), ("Simple Twist of Fate", 279), ("Shelter from the Storm", 302)]),
    ("Joni Mitchell", "Blue", [("A Case of You", 261), ("River", 240), ("California", 228), ("Blue", 183)]),
    ("Pink Floyd", "The Dark Side of the Moon", [("Time", 413), ("Money", 382), ("Us and Them", 469), ("Breathe", 163)]),
    ("David Bowie", "Hunky Dory", [("Life on Mars?", 232), ("Changes", 217), ("Oh! You Pretty Things", 192)]),
    ("Kate Bush", "Hounds of Love", [("Running Up That Hill", 300), ("Cloudbusting", 302), ("Hounds of Love", 182)]),
    ("Prince", "Purple Rain", [("Purple Rain", 520), ("When Doves Cry", 352), ("Let's Go Crazy", 279)]),
    ("Stevie Nicks", "Bella Donna", [("Edge of Seventeen", 328), ("Leather and Lace", 375)]),
    ("Tracy Chapman", "Tracy Chapman", [("Fast Car", 296), ("Talkin' Bout a Revolution", 158), ("Baby Can I Hold You", 191)]),
    ("Norah Jones", "Come Away with Me", [("Don't Know Why", 187), ("Come Away with Me", 198), ("Turn Me On", 154)]),
    ("Amy Winehouse", "Back to Black", [("Rehab", 214), ("Back to Black", 240), ("Love Is a Losing Game", 154)]),
    ("Adele", "21", [("Rolling in the Deep", 228), ("Someone Like You", 285), ("Set Fire to the Rain", 242)]),
    ("Lana Del Rey", "Norman Fucking Rockwell!", [("Mariners Apartment Complex", 244), ("Venice Bitch", 583), ("Doin' Time", 201)]),
    ("Mitski", "Be the Cowboy", [("Nobody", 210), ("Washing Machine Heart", 126), ("Two Slow Dancers", 268)]),
    ("Phoebe Bridgers", "Punisher", [("Kyoto", 185), ("Punisher", 199), ("I Know the End", 348)]),
    ("Frank Ocean", "Blonde", [("Nikes", 314), ("Ivy", 249), ("Self Control", 249), ("Nights", 307)]),
    ("Tyler, The Creator", "IGOR", [("EARFQUAKE", 190), ("I THINK", 189), ("A BOY IS A GUN", 205)]),
    ("Solange", "A Seat at the Table", [("Cranes in the Sky", 250), ("Don't Touch My Hair", 245)]),
    ("D'Angelo", "Voodoo", [("Untitled (How Does It Feel)", 434), ("Devil's Pie", 265)]),
    ("Erykah Badu", "Baduizm", [("On & On", 226), ("Next Lifetime", 359), ("Otherside of the Game", 456)]),
    ("The Cure", "Disintegration", [("Pictures of You", 447), ("Lovesong", 208), ("Plainsong", 311)]),
    ("Joy Division", "Unknown Pleasures", [("Disorder", 202), ("She's Lost Control", 237), ("Shadowplay", 234)]),
    ("New Order", "Power, Corruption & Lies", [("Age of Consent", 305), ("Blue Monday", 448)]),
    ("The Smiths", "The Queen Is Dead", [("There Is a Light That Never Goes Out", 243), ("Bigmouth Strikes Again", 192)]),
    ("Pixies", "Doolittle", [("Here Comes Your Man", 202), ("Debaser", 172), ("Monkey Gone to Heaven", 177)]),
    ("Arcade Fire", "Funeral", [("Wake Up", 335), ("Rebellion (Lies)", 310), ("Neighborhood #1", 288)]),
    ("Sufjan Stevens", "Illinois", [("Chicago", 361), ("Casimir Pulaski Day", 355)]),
    ("Bon Iver", "For Emma, Forever Ago", [("Skinny Love", 238), ("Flume", 199), ("Re: Stacks", 385)]),
    ("The National", "High Violet", [("Bloodbuzz Ohio", 275), ("Terrible Love", 279)]),
    ("Caribou", "Swim", [("Odessa", 366), ("Sun", 320), ("Jamelia", 328)]),
    ("Four Tet", "Rounds", [("She Moves She", 296), ("As Serious as Your Life", 269)]),
    ("Jamie xx", "In Colour", [("Gosh", 300), ("Loud Places", 265), ("Sleep Sound", 254)]),
    ("Floating Points", "Promises", [("Movement 1", 250), ("Movement 6", 342)]),
    ("Nils Frahm", "Spaces", [("Says", 528), ("An Aborted Beginning", 199)]),
    ("Ólafur Arnalds", "re:member", [("re:member", 258), ("unfold", 236)]),
    ("Ryuichi Sakamoto", "async", [("andata", 400), ("solari", 273)]),
    ("Toro y Moi", "Anything in Return", [("So Many Details", 285), ("Say That", 231)]),
    ("Tame Impala", "Currents", [("Let It Happen", 467), ("The Less I Know the Better", 216)]),
    ("Rosalía", "El Mal Querer", [("MALAMENTE", 143), ("PIENSO EN TU MIRÁ", 194), ("BAGDAD", 199)]),
    ("Caetano Veloso", "Transa", [("You Don't Know Me", 285), ("Triste Bahia", 396)]),
    ("Gal Costa", "Gal Costa", [("Baby", 190), ("Sebastiana", 172)]),
    ("Cesária Évora", "Miss Perfumado", [("Sodade", 268), ("Angola", 245)]),
    ("Ali Farka Touré", "Talking Timbuktu", [("Diaraby", 476), ("Soukora", 400)]),
    ("Tinariwen", "Aman Iman", [("Cler Achel", 267), ("Matadjem Yinmixan", 258)]),
    ("Ravi Shankar", "The Sounds of India", [("Raga Jog", 900), ("Raga Simhendramadhyamam", 720)]),
    ("Nusrat Fateh Ali Khan", "Shahen-Shah", [("Allah Hoo Allah Hoo", 600), ("Shamas-Ud-Doha", 660)]),
    ("Antonio Carlos Jobim", "Wave", [("Wave", 175), ("Triste", 155)]),
    ("Chet Baker", "Chet Baker Sings", [("My Funny Valentine", 143), ("But Not for Me", 178)]),
    ("John Coltrane", "A Love Supreme", [("Acknowledgement", 452), ("Resolution", 442)]),
    ("Thelonious Monk", "Brilliant Corners", [("Brilliant Corners", 464), ("Bemsha Swing", 314)]),
    ("Ella Fitzgerald", "Ella and Louis", [("Cheek to Cheek", 349), ("Summertime", 292)]),
    ("Duke Ellington", "Money Jungle", [("Money Jungle", 328), ("Fleurette Africaine", 234)]),
    ("Arvo Pärt", "Tabula Rasa", [("Spiegel im Spiegel", 540), ("Fratres", 660)]),
    ("Philip Glass", "Glassworks", [("Opening", 380), ("Floe", 350)]),
    ("Steve Reich", "Music for 18 Musicians", [("Pulses", 320), ("Section I", 340)]),
    ("Gustav Mahler", "Symphony No. 5", [("Adagietto", 600), ("Trauermarsch", 780)]),
    ("Sergei Rachmaninoff", "Piano Concerto No. 2", [("Moderato", 660), ("Adagio sostenuto", 690)]),
]
