/** Tapedeck API client — cookie-based session auth for browser, Token header for external clients. */

const BASE = '';

export interface Scrobble {
	id: number;
	title: string;
	artist: string;
	album: string | null;
	/** Who this was heard with, or null for not marked. Null means nobody
	 *  recorded it — never that they were definitely alone. */
	company: string | null;
	/** The Shared Spool this belonged to, when it came from a sitting. */
	joint_session_id: string | null;
	timestamp: number;
	duration: number | null;
	status: string;
	format_type: string | null;
	codec: string | null;
	bitrate: number | null;
	sample_rate: number | null;
	bit_depth: number | null;
	container: string | null;
	/** What the player claimed it was handed, kept apart from the measurements
	 *  above because the two can disagree. */
	source_quality: string | null;
	is_lossless: boolean | null;
	dsd_rate: number | null;
	dsd_multiplier: number | null;
	/** A DSD stream decoded to PCM before it reached the output — a change of
	 *  representation rather than a bitrate compromise, and the thing a
	 *  DSD-capable DAC exists to avoid. */
	dsd_to_pcm_converted: boolean | null;
	delivery_codec: string | null;
	delivery_sample_rate: number | null;
	delivery_bit_depth: number | null;
	quality_score: number | null;
	chain_id: number | null;
	/** Which setup of that chain was fitted, when one was named. */
	chain_variant_id: number | null;
	listening_context: string | null;
	submission_client: string | null;
	/** Which code path produced it: Plex, jellyfin, emby, navidrome, physical,
	 *  rockbox, import, or `ingest:<client>`. */
	source_name: string | null;
	mbid_recording: string | null;
	mbid_release: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
	/** Resolved cover image from whichever provider answered. */
	artwork_url: string | null;
}

/** Your listening placed on Every Noise's genre map. */
export interface GenreMap {
	/** The backdrop: every genre as a compact [name, x, y, colour] tuple. */
	map: { width: number; height: number; points: [string, number, number, string][] };
	/** The caller's own genres, weighted by plays. */
	mine: { name: string; plays: number; x: number; y: number; color: string }[];
	/** Play-weighted average colour of the above — "what your listening sounds like". */
	colour: string | null;
	placed_tags: number;
	/** Plays on tags with no place on the map (instruments, nationalities). */
	unplaced_plays: number;
	listens_with_genres: number;
	listens_total: number;
}

/** A genre adjacent to the caller's listening. */
export interface NeighbourGenre {
	name: string;
	x: number;
	y: number;
	color: string;
	/** How many of your top genres border it. */
	borders: number;
	affinity: number;
	your_plays: number;
	/** Strictly your own artists, from your own history. Always empty for
	 *  `unexplored`, which is what puts a genre in that half. */
	your_artists: { artist: string; plays: number }[];
	/** Every Noise's own exemplars, set only where `your_artists` is empty.
	 *  **These name music you do not own** — anything rendering them has to say
	 *  so. Empty until a tier-2 scrape has run. */
	exemplar_artists: string[];
}

export interface GenreNeighbours {
	/** Bordering genres you already own music in, and underplay. */
	next_door: NeighbourGenre[];
	/** Bordering genres you own nothing in — a region, never a suggestion. */
	unexplored: NeighbourGenre[];
	/** Genres your listening is furthest from. */
	opposite: NeighbourGenre[];
	seeded_from: number;
}

/** Play-weighted centre of each year's listening, on the same map coordinates. */
export interface GenreTrajectory {
	years: {
		year: string;
		x: number;
		y: number;
		plays: number;
		colour: string | null;
		/** Listens that year, and how many have genres — a year still being
		 *  enriched is drawn faintly rather than presented as complete. */
		listens: number;
		enriched: number;
	}[];
	/** Which bucket the server chose — 'year', or 'month' for a short history. */
	unit?: 'year' | 'month';
	/** Periods below this many placed plays are omitted entirely. */
	min_plays?: number;
	note?: string;
}

/** State of the local copy of the Every Noise genre map, and any running scrape. */
export interface SinkHealth {
	name: string;
	failures: number;
	total_failures: number;
	total_successes: number;
	last_ok: number | null;
	last_failure: number | null;
	last_error: string | null;
	/** Set while the breaker is open and this service is being skipped outright. */
	skipping_for: number | null;
}

export interface SourceHealth {
	id: number;
	user_id: number;
	username: string;
	kind: string;
	label: string;
	enabled: boolean;
	poll_interval_secs: number | null;
	last_polled_at: number | null;
	last_error: string | null;
}

export interface OpsStatus {
	version: string;
	uptime_secs: number;
	logs_enabled: boolean;
	tick: {
		interval_secs: number;
		ticks: number;
		last_started: number | null;
		last_duration_ms: number;
		longest_ms: number;
		overruns: number;
		deferred_listens: number;
		/** The tick is taking longer than its own interval — sources and the deck are that late. */
		behind: boolean;
	};
	sinks: SinkHealth[];
	sources: SourceHealth[];
}

export interface LogFile {
	name: string;
	bytes: number;
}

export interface LogTail {
	file: string;
	files: string[];
	/** What the filter found, against `shown` — so a capped tail cannot read as the whole answer. */
	matched: number;
	shown: number;
	truncated: boolean;
	lines: string[];
}

export interface EveryNoiseStatus {
	summary: {
		genres: number;
		artists: number;
		edges: number;
		/** Genre pages walked, out of `genres`. */
		pages_scraped: number;
		last_scraped_at: number | null;
	};
	progress: {
		tier: 'map' | 'pages';
		state: 'running' | 'done' | 'failed' | 'cancelled';
		done: number;
		total: number;
		current: string | null;
		message: string | null;
		started_at: number;
	} | null;
	/** The admin's contact address, sent in the scraper's User-Agent. */
	contact: string | null;
	source: string;
}

export interface DashboardStats {
	today: number;
	this_week: number;
	total: number;
	today_hours: number;
	week_hours: number;
	lossless_pct: number;
	/** What `lossless_pct` is a share *of* — listens whose format is known, not
	 *  every play. See `FIDELITY_KNOWN` in `db.rs`. */
	fidelity_known: number;
	top_artist: string;
	top_artist_count: number;
	unique_artists: number;
	unique_albums: number;
	unique_tracks: number;
	total_hours: number;
	avg_quality: number;
}

export type StatsRange = 'week' | 'month' | 'last30' | 'year' | 'all';

export interface RangedStats {
	totals: {
		plays: number; hours: number; artists: number; albums: number; tracks: number;
		avg_quality: number; lossless_pct: number; dsd_pct: number;
		/** The denominator of the two shares above — not `plays`. */
		fidelity_known: number;
		/** What `avg_quality` is an average *of*. Same narrow population the
		 *  fidelity shares have: only a live source ever writes a quality
		 *  score, so an imported history has none. */
		quality_known: number;
		/** What `hours` is summed over — a listen of unknown length adds nothing,
		 *  so the figure is a floor rather than a total. */
		duration_known: number;
		/** Skipped plays in the range. The one figure here that counts them. */
		skips: number;
		/** Plays per **calendar** day of the range, not per day you listened. */
		per_day: number;
		/** Earliest listen, unix seconds. Whole history — never ranged. */
		first_listen: number | null;
		/** Loved entities: tracks, records and artists together. */
		loves: number;
	};
	top_artists: { name: string; plays: number; hours: number; image_url: string | null }[];
	top_albums: { name: string; artist: string; plays: number; hours: number }[];
	top_tracks: { title: string; artist: string; album: string | null; plays: number; hours: number }[];
	fidelity: { name: string; plays: number; pct: number }[];
	/** A track carries several tags, so these are shares of tag mentions, not of listens. */
	genres: { name: string; plays: number; pct: number }[];
	/** Present once the genre map is fetched and genres are backfilled. */
	breadth?: {
		genres_played: number;
		genres_placed: number;
		/** exp(Shannon entropy): "as varied as playing N genres equally". */
		effective_genres: number;
		/** effective / played; 1.0 would be every genre played equally. */
		evenness: number;
		/** Your spread over the map's own spread. Null if nothing is placed. */
		reach: number | null;
	};
	languages: { name: string; pct: number }[];
	regions: { name: string; pct: number }[];
	decades: { name: string; plays: number; pct: number }[];
	heatmap: number[][]; // 7 days × 24 hours
}

/** One period card on the Reports feed. */
export interface ReportPeriod {
	key: string;
	label: string;
	start: number;
	end: number;
	plays: number;
	hours: number;
	/** Plays per calendar day — full length for a finished period, days elapsed
	 *  for one still running. */
	avg_daily: number;
	artists: number;
	albums: number;
	tracks: number;
	new_artists: number;
	top_artist: {
		name: string;
		plays: number;
		hours: number;
		/** Set by hand on the artist page; absent for most, which is why the
		 *  chapter card keeps its big initial as the design. */
		landscape_url?: string | null;
	} | null;
	top_album: { name: string; artist: string; plays: number } | null;
	top_track: { title: string; artist: string; plays: number } | null;
	/** 24 counts, bucketed in your own timezone. */
	clock: number[];
	/** Plays per sub-bucket: days across a week, weeks across a month, months
	 *  across a year. */
	sub: number[];
	sub_labels: string[];
	/** Shares of **tag mentions**, not of listens — say so wherever shown. */
	genres: { name: string; pct: number }[];
	/** The period's three biggest genres plus "other", and each one's share in
	 *  every sub-bucket. Drawn as stacked bands by the year chapter. The names
	 *  are fixed across the whole period so the bands keep their identity. */
	genre_bands: { names: string[]; shares: number[][] };
	/** Where this period sits among all of *your own* periods by plays, 1 being
	 *  the busiest. Not a cross-user percentile — see `reports.rs`. */
	rank: number;
	rank_of: number;
	/** Play-weighted average of the period's genre colours. Null until the
	 *  genre map has been fetched. */
	colour: string | null;
	on_repeat: { title: string; artist: string; plays: number }[];
	/** The most-played artist you had never played before. Absent for the
	 *  oldest period, where everything is new by construction. */
	crush: { name: string; plays: number; since: number } | null;
	shelf: { sides: number; rows: { title: string; artist: string; side: string; spins: number }[] };
	top_chain: string | null;
	/** What you listened *through* — the pillar no other card on the page can
	 *  show, since every other one is about *what* played.
	 *
	 *  `unattributed` and `no_duration` are stated rather than folded away: on
	 *  most histories the first is nearly everything, and the hours are a
	 *  **floor** without the second. A panel that hid them would read as a
	 *  complete account of the period instead of an account of the part of it
	 *  somebody attributed. */
	gear: {
		chains: { name: string; icon: string | null; plays: number; hours: number }[];
		items: { name: string; type: string; plays: number; hours: number }[];
		unattributed: number;
		no_duration: number;
	};
	/** Listens **marked** as heard with somebody.
	 *
	 *  Marked, not "social": `company` is null on everything nobody has tagged,
	 *  which is almost all listening, so reading null as "alone" would be
	 *  answering "how much of this was *recorded* as social" while appearing to
	 *  answer "how much of this *was*". `of_plays` is the denominator, and it
	 *  travels so nothing can present a share without saying what of. */
	company: {
		marked_plays: number;
		marked_hours: number;
		of_plays: number;
		people: { name: string; plays: number }[];
	};
	/** The period's shape, as five numbers between 0 and 1, against the same
	 *  five for the reader's own usual.
	 *
	 *  **Every axis is answerable from a time, an artist, a title and an album**
	 *  — deliberately, because the tempting axes (signal chain, format,
	 *  attention) are unknown for most of a real history, so a shape drawn from
	 *  them would picture how the listening was *recorded* while looking like a
	 *  picture of the listening. Those live in `gear`, where a panel can state
	 *  its own coverage; a radar cannot legibly carry five denominators.
	 *
	 *  `usual` is the mean of the reader's own periods — never a comparison
	 *  against other people. `usual_periods` says how many it rests on, so a
	 *  baseline of one is not drawn as though it were a habit. */
	fingerprint: {
		axes: string[];
		you: number[];
		usual: number[];
		usual_periods: number;
	};
	milestones: string[];
	prev: {
		label: string; plays: number; hours: number; avg_daily: number;
		artists: number; albums: number; tracks: number; sub: number[];
	} | null;
}

/** One row on the Loved page: the entity, plus what your own history says
 *  about it. A love is an entity, and entities come in three kinds — `album`
 *  and the artwork fields are absent for an artist. */
export interface LovedRow {
	/** Narrower than `Entity` on purpose — a loves listing has no use for
	 *  `created_at`, and sending it would only invite it being read as "when
	 *  you loved this", which is `loved_at`. */
	entity: Pick<Entity, 'id' | 'kind' | 'name' | 'artist_name' | 'mbid'>;
	loved_at: number;
	plays: number;
	last_played: number | null;
	album: string | null;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
	note_id: number | null;
	/** First ~90 characters of the liner note, if there is one. */
	note_excerpt: string | null;
}

export interface Reports {
	unit: 'week' | 'month' | 'year';
	timezone: string;
	periods: ReportPeriod[];
	/** Where this page sits, counting back from the newest period. */
	offset: number;
	/** How many periods exist at this bucket size — what tells the stepper
	 *  where the far end is. */
	total_periods: number;
	/** True when the history hit the analytics row cap — the oldest periods are
	 *  then incomplete and should not be presented as final. */
	capped?: boolean;
}

export interface PersonalityTrait {
	label: string;
	low: string;
	high: string;
	score: number; // 0..100, 50 = balanced
	evidence: string;
}
export interface Personality {
	verdict: string;
	traits: PersonalityTrait[];
	loyalty: { artist: string; plays: number; loyalty_score: number }[];
	total_plays: number;
}

export interface ImportJob {
	job_id: string; source: string; state: 'running' | 'done' | 'error';
	fetched: number; imported: number; total: number | null; status: string; date: number;
	/** Listens already held that the incoming copy filled something in on — an
	 *  MBID, a duration, a cover-art id. A duplicate is no longer dropped
	 *  whole: blank fields are filled from it, populated ones never touched. */
	enriched?: number;
}

export interface ImportRecord {
	source: string; date: number; count: number; status: string;
}

export interface SignalChain {
	id: number;
	name: string;
	description: string | null;
	components: ChainComponent[];
	listening_context: string;
	/** A glyph name from ChainIcon's set; unknown or absent renders generic. */
	icon?: string | null;
	is_active: boolean;
	/** Whether listens through this chain reach the Reel of anyone patched into
	 *  you. **The only sharing switch there is** — there is no per-listen
	 *  visibility. Off by default, including for every chain that predates it. */
	shared: boolean;
	total_hours: number;
}

export interface ChainComponent {
	type: string;
	name: string;
	detail: string | null;
	/** The `Equipment` row this step is, when picked from your gear. `name` is
	 *  kept as a snapshot so the chain still reads if the gear is deleted. */
	equipment_id?: number | null;
	/** Unix seconds from which this box was in the path, or null for "as long
	 *  as the chain has existed" — which is what every step written before this
	 *  field meant. Chain hours are derived from the listens, so without it a
	 *  box added to a chain that already has history is credited with all of
	 *  it. Stamped by the server on gear a chain did not name before. */
	since?: number | null;
}

export interface ChainInput {
	name: string;
	description?: string;
	components: ChainComponent[];
	listening_context?: string;
	icon?: string | null;
}

/**
 * The owner's own view of their public now-playing switch.
 *
 * `known_fields` comes from the server rather than being hardcoded here, so a
 * build that learns a fifth disclosure renders its toggle without a frontend
 * change — and, more to the point, so this screen can never offer a switch the
 * server would silently ignore.
 */
export interface PublishSwitch {
	enabled: boolean;
	fields: string[];
	known_fields: string[];
	path: string;
}

/** An artist seen across the decks you've patched into. */
export interface PatchArtist {
	name: string;
	plays: number;
	decks: number;
	who: string | null;
	last_played: number | null;
	/** When they first appeared in the shared history — what "newest arrival"
	 *  sorts on. */
	first_played: number | null;
	/** Their own tag, from the cache the genre map fills. Null for anyone
	 *  MusicBrainz has not tagged — a normal state, drawn as nothing. */
	genre: string | null;
	/** Your own plays. **The overlap is which artists, never a score.** */
	mine: number;
}

export interface DeckChain {
	name: string;
	icon: string | null;
	components: string;
	hours: number;
}

/** Somebody else's deck, seen through an accepted patch. */
export interface DeckView {
	user_id: number;
	username: string;
	display_name: string | null;
	bio: string | null;
	avatar_url: string | null;
	created_at: number;
	/** Listens through their **shared** chains — not their history. */
	listens: number;
	chains: DeckChain[];
	both_play: string[];
	/** Null when they have no ActivityPub identity, which is a normal state. */
	actor: string | null;
}

export interface DiscoverDeck {
	user_id: number;
	username: string;
	display_name: string | null;
	bio: string | null;
	avatar_url: string | null;
	created_at: number;
	shared_chains: number;
	state: string | null;
	/** Unit-separated (\x1f) artists you both play. */
	overlap: string;
}

export interface InstancePulse {
	decks: number;
	publishing: number;
	shared_chains: number;
	listens: number;
	hours: number;
	/** Distinct artists heard in the window. **Not `artists.length`** — that
	 *  list is cut to the top 8 for the panel. */
	artists_heard: number;
	artists: PatchArtist[];
	chains: { name: string; plays: number }[];
	/** Raw (timestamp, hours) pairs — the day boundary is the reader's. */
	daily: { at: number; hours: number }[];
}

export interface FederationOps {
	identities: number;
	published: number;
	frozen: number;
	remote_actors: number;
	inbox_activities: number;
	deliveries: number;
}

/** A record somebody handed you. */
export interface Dub {
	id: number;
	kind: string;
	artist: string;
	name: string | null;
	reason: string | null;
	because: string | null;
	status: string;
	created_at: number;
	from_username: string;
	from_display_name: string | null;
}

export interface SpoolMember {
	user_id: number;
	username: string;
	display_name: string | null;
	/** invited | joined | left | declined.
	 *
	 *  Leaving a sitting you were in and saying no to an invitation are
	 *  different answers, and only one of them means you were there. */
	state: string;
	/** When they were in the room from, or null for the whole sitting — which
	 *  is the common case, not a missing value. */
	present_from: number | null;
	/** When they left, or null for stayed to the end. */
	present_until: number | null;
}

/** One sitting on the history list. */
export interface SpoolSummary {
	uuid: string;
	title: string | null;
	notes: string | null;
	started_at: number;
	ended_at: number | null;
	hosting: boolean;
	host_username: string;
	host_display_name: string | null;
	/** Who else was actually in the room — you are left out.
	 *
	 *  `SpoolDetail` keeps everybody and hands over a `viewer_id`, because a
	 *  page listing attendance has to show you your own row. This has one use:
	 *  the sentence "with Noor and Tobias". */
	present: string[];
	/** Names with no account here. */
	guests: string[];
	plays: number;
	pending: number;
}

/** One play on a sitting's timeline, whoever's deck it came off. */
export interface SpoolPlay {
	title: string;
	artist: string;
	album: string | null;
	duration: number | null;
	played_at: number;
	from_username: string;
	from_display_name: string | null;
	/** `mine` | `accepted` | `declined` | `pending` | `theirs`.
	 *
	 *  Declined and never-answered are told apart deliberately: both mean "not
	 *  in my history" and they are not the same decision. */
	standing: string;
	capture_id: number | null;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
}

/** What an evening came to. */
export interface SpoolFigures {
	plays: number;
	/** Running time of the plays that reported one, and how many did. **Both
	 *  travel or neither does** — a source that reported no length contributes
	 *  nothing, so this is a floor. */
	seconds: number;
	duration_known: number;
	artists: number;
	albums: number;
	top_artist: [string, number] | null;
	top_album: [string, string, number] | null;
	/** Whose deck played how many, biggest first. */
	decks: [string, number][];
}

/** A sitting, in full. */
export interface SpoolDetail {
	uuid: string;
	viewer_id: number;
	title: string | null;
	notes: string | null;
	started_at: number;
	ended_at: number | null;
	host_user_id: number;
	hosting: boolean;
	my_state: string;
	members: SpoolMember[];
	guests: string[];
	plays: SpoolPlay[];
	/** Plays that happened here and are not shown, because they ran through
	 *  chains their owner does not share. Stated rather than dropped — a
	 *  timeline with invisible holes misrepresents the evening. */
	withheld: number;
	/** What the evening came to, folded out of `plays`. */
	figures: SpoolFigures;
}

export interface SpoolView {
	uuid: string;
	/** Who is asking. `members` includes you, and a line saying "with Noor and
	 *  Tobias" has to leave you out of it. */
	viewer_id: number;
	title: string | null;
	started_at: number;
	/** Null while it is running. An **ended** sitting still arrives here when
	 *  you were invited and never answered — an invitation is outstanding until
	 *  it is answered, and the host closing the room does not answer it. */
	ended_at: number | null;
	host_user_id: number;
	hosting: boolean;
	my_state: string;
	members: SpoolMember[];
	/** Names with no account here, so a banner can say who else is in the room
	 *  rather than only who else has a deck. */
	guests: string[];
	captured: number;
}

/** A deck on this instance that plays a given artist. */
export interface ArtistDeck {
	user_id: number;
	username: string;
	display_name: string | null;
	avatar_url: string | null;
	actor_uri: string | null;
	/** Their plays, **through shared chains only**. */
	plays: number;
	/** Whether you already have an accepted patch, either direction — which
	 *  decides whether their deck can be opened or has to be asked for. */
	patched: boolean;
}

/** A deck you're patched into, playing something right now.
 *
 *  Gated by exactly the rule the Reel uses — an accepted patch, and then
 *  either the deck has opened up or the listen ran through a chain they share.
 *  `chain_name` is therefore nullable: `null` with `has_chain` means "Private
 *  signal chain", and `null` without it means the listen ran through nothing
 *  anybody recorded. Two different facts, drawn differently. */
export interface LiveDeck {
	user_id: number;
	username: string;
	display_name: string | null;
	avatar_url: string | null;
	title: string;
	artist: string;
	album: string | null;
	duration: number | null;
	position: number;
	/** False when the position is wall-clock arithmetic rather than a reported
	 *  playhead. Drawn as an estimate, never as a measurement. */
	position_known: boolean;
	paused: boolean;
	/** Named only when its owner shares the chain — see the note above. */
	chain_name: string | null;
	/** Whether there was a chain at all, whatever its owner shares. */
	has_chain: boolean;
	artwork_url: string | null;
	format_type: string | null;
	codec: string | null;
	bit_depth: number | null;
	sample_rate: number | null;
	is_lossless: boolean | null;
}

/** A sitting somebody handed you. **A document, never captures** — it enters
 *  no history, count or chart of yours, so there is nothing to accept. */
/** One sitting **you** passed on — see `getSentSittings`. */
export interface SentSitting {
	uuid: string;
	session_uuid: string;
	note: string | null;
	created_at: number;
	/** When they opened it. `null` is still unread. */
	seen_at: number | null;
	to_username: string;
	to_display_name: string | null;
	title: string | null;
	started_at: number | null;
	ended_at: number | null;
}

export interface HandedSitting {
	uuid: string;
	session_uuid: string;
	note: string | null;
	created_at: number;
	seen_at: number | null;
	from_username: string;
	from_display_name: string | null;
	title: string | null;
	started_at: number | null;
	ended_at: number | null;
}

export interface HandedListen {
	title: string;
	artist: string;
	album: string | null;
	timestamp: number;
	duration: number | null;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
	chain_name: string | null;
}

export interface HandedSittingDetail {
	uuid: string;
	note: string | null;
	from_username: string;
	from_display_name: string | null;
	title: string | null;
	started_at: number;
	ended_at: number | null;
	listens: HandedListen[];
	/** Listens not shown because they ran through chains the owner doesn't
	 *  share. Stated rather than silently dropped. */
	omitted: number;
	/** The sitting was regrouped out of existence since it was handed over. */
	gone: boolean;
}

/** A play from somebody else's deck. **Not a listen** until accepted. */
export interface SpoolCapture {
	id: number;
	title: string;
	artist: string;
	album: string | null;
	duration: number | null;
	played_at: number;
	state: string;
	from_username: string;
	from_display_name: string | null;
	spool_uuid: string;
	spool_title: string | null;
}

/** The other end of a patch — or a deck nobody has asked about yet. */
export interface PatchPeer {
	/** The patch's own id. Null when no edge exists — which is what tells
	 *  "not patched" apart from "asked and waiting". */
	uuid: string | null;
	/** `pending` | `accepted` | `rejected`, or null for no edge. */
	state: string | null;
	created_at: number | null;
	user_id: number;
	username: string;
	display_name: string | null;
	bio: string | null;
	avatar_url: string | null;
	/** How many chains this person publishes. **Read it with
	 *  `shares_everything`**, which is the gate: a deck sharing no chains but
	 *  open to its patched decks shows everything it plays. Zero chains *and*
	 *  closed is the empty patch that needs saying rather than looking broken. */
	shared_chains: number;
	/** Whether they let decks they have patched in see everything they play,
	 *  rather than only listens through a chain they publish. */
	shares_everything: boolean;
}

export interface PatchDecks {
	decks: PatchPeer[];
	following: PatchPeer[];
	followers: PatchPeer[];
}

/** A listen on someone else's deck, reaching you through a patch. */
export interface ReelListen {
	id: number;
	timestamp: number;
	title: string;
	artist: string;
	album: string | null;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
	format_type: string | null;
	codec: string | null;
	bitrate: number | null;
	sample_rate: number | null;
	bit_depth: number | null;
	is_lossless: boolean | null;
	submission_client: string | null;
	user_id: number;
	username: string;
	display_name: string | null;
	avatar_url: string | null;
	/** Their actor URI, or null when they haven't switched federation on.
	 *  The handle is derived from **this**, never from today's origin — a deck
	 *  with no actor has no published address, and showing one would invent
	 *  something that resolves nowhere. */
	actor_uri: string | null;
	/** The chain, **when its owner shares it**.
	 *
	 *  `null` has two meanings and the row must not conflate them, which is
	 *  what `has_chain` is for: a chain kept private reads "Private signal
	 *  chain", and a listen that ran through no chain at all says nothing. */
	chain_name: string | null;
	chain_icon: string | null;
	chain_components: string | null;
	/** Whether there was a chain, whatever its owner shares. */
	has_chain: boolean;
}

export interface ReelPage {
	listens: ReelListen[];
	/** Keyset cursor: pass as `before`. Null when the page was short. */
	next: number | null;
}

/**
 * Whether this account answers to the rest of the fediverse.
 *
 * `actor` is null until a keypair has been minted, which happens the moment the
 * switch goes on — `would_be` is what it will become, shown as a preview rather
 * than asserted as an address. `frozen_at` is non-null once the username is
 * load-bearing for another server and must not be renamed.
 */
export interface FederationSwitch {
	enabled: boolean;
	handle: string;
	actor: string | null;
	would_be: string;
	frozen_at: number | null;
}

export interface EquipmentInput {
	name: string;
	type: string;
	brand?: string;
	model?: string;
	notes?: string;
	/** When it was bought, unix seconds. Distinct from `first_used`, which is
	 *  when Tapedeck first saw it. */
	purchased_at?: number | null;
	/** Why it is not in a chain — `new`, `spare` or `lent`. Accepted on update
	 *  only; the create endpoint has no field for it, since gear is tagged from
	 *  the "Not in a chain yet" table once it exists.
	 *
	 *  Note the update endpoint is a whole-record replace, so every caller has
	 *  to send this back or tagging a spare clears it. */
	status?: string | null;
}

export interface Connection {
	service: string;
	username: string | null;
	connected_at: number;
}

export interface Binding {
	id: number;
	identifier: string;
	interface: string | null;
	chain_id: number | null; // null = detected but unmapped
	last_seen: number;
	hits: number;
}

export interface Device {
	id: number;
	machine_id: string;
	name: string | null;
	platform: string | null;
	product: string | null;
	device_type: string | null;
	default_chain_id: number | null;
	total_listens: number;
	last_seen: number;
}

/** Your own account. `avatar` is a URL the browser can load directly — either
 *  the external one you set or the endpoint that serves an uploaded file. The
 *  path on disk is deliberately never sent. */
export interface UserProfile {
	id: number;
	username: string;
	display_name: string | null;
	role: 'admin' | 'user';
	created_at: number;
	bio: string | null;
	avatar: string | null;
	disabled: boolean;
}

/** One step of a chain, replaced.
 *
 *  `position` indexes into the parent chain's `components`. Only the steps that
 *  differ are stored, so editing the shared part of a chain updates every
 *  variant of it — which is why two cartridges on one turntable stay one chain.
 *  A position past the end *adds* a step. */
export interface VariantOverride {
	position: number;
	equipment_id?: number | null;
	name?: string | null;
}

/** A chain with one or more steps swapped.
 *
 *  `total_hours` is the listening that went through *this variant*, which is a
 *  different question from the chain's total — a chain's hours include every
 *  variant of it, since they all ran through the same chain. */
export interface ChainVariant {
	id: number;
	user_id: number;
	chain_id: number;
	name: string;
	overrides: VariantOverride[];
	created_at: number;
	total_hours: number;
}

/** Two chains that look like the same rig, and what folding one into the other
 *  would do.
 *
 *  Reported, never acted on — a fold is not reversible, so the pair, the
 *  distance and the overrides are all shown before anything is agreed to. */
export interface FoldSuggestion {
	base_chain_id: number;
	base_name: string;
	base_hours: number;
	fold_chain_id: number;
	fold_name: string;
	fold_hours: number;
	/** How many steps disagree. 0 is the same wiring under two names, 1 is the
	 *  swapped-box case variants exist for. */
	distance: number;
	/** What the folded chain becomes as a variant of the base. */
	overrides: VariantOverride[];
	/** `[id, name]` of a variant of the base that is already wired this way —
	 *  which is the state you are in when you built the variant and the old
	 *  chain is still sitting in the list beside it. */
	matching_variant: [number, string] | null;
	/** This one carries variants of its own, so it cannot be the side that
	 *  folds — their positions index into components the fold replaces. */
	fold_blocked_by_variants: boolean;
}

/** What a fold actually moved. Reported per table because they are different
 *  kinds of thing: listens are the hours, a token or device default is *future*
 *  attribution, and a release is the turntable a record lives with. */
export interface FoldOutcome {
	variant_id: number;
	variant_name: string;
	variant_created: boolean;
	listens: number;
	sides: number;
	sessions: number;
	releases: number;
	devices: number;
	bindings: number;
	tokens: number;
}

/** Where one piece of gear's hours came from.
 *
 *  A total is a sum, and a sum is not checkable — with a variant in play the
 *  listening genuinely arrives from more than one chain, so "4h" is a number
 *  you either believe or you don't. */
export interface GearHours {
	chain_id: number;
	chain_name: string;
	/** Set when it was fitted as a *variant* of that chain — the case this
	 *  exists for, since that is when gear earns hours through a chain whose
	 *  own components do not name it. */
	variant_id: number | null;
	variant_name: string | null;
	hours: number;
}

export interface Equipment {
	id: number;
	name: string;
	equipment_type: string;
	brand: string | null;
	model: string | null;
	total_hours: number;
	first_used: number | null;
	last_used: number | null;
	notes: string | null;
	purchased_at: number | null;
	/** `new`, `spare` or `lent`, or null when nothing has been said. Only ever
	 *  meaningful for gear no chain names. */
	status: string | null;
	/** Which chains and variants `total_hours` is made of, largest first. Empty
	 *  for gear nothing has logged time through. */
	sources: GearHours[];
}

/** Live playback state. Field names mirror `Scrobble` so `qualityLabel()` works on it. */
export interface NowPlayingTrack {
	title: string;
	artist: string;
	album: string | null;
	duration: number | null;
	progress: number;
	/**
	 * False when the source cannot see a playhead and `progress` is the
	 * server's own wall-clock arithmetic — a ListenBrainz `playing_now` carries
	 * no position in the format at all, and Subsonic reports `minutesAgo`. Draw
	 * those as an estimate; a pause or a seek makes them arbitrarily wrong.
	 */
	position_known: boolean;
	updated_at: number;
	source: string;
	listening_context: string | null;
	format_type: string | null;
	codec: string | null;
	bit_depth: number | null;
	sample_rate: number | null;
	dsd_multiplier: number | null;
	delivery_codec: string | null;
	is_lossless: boolean | null;
}

export interface NowPlayingState {
	playing: boolean;
	/** Seconds into the track, as of the moment the server answered. */
	position?: number;
	track?: NowPlayingTrack;
}

/**
 * One of the caller's own polling sources. Credentials are never sent back —
 * `has_token` is all the UI gets.
 */
export interface UserSource {
	id: number;
	kind: string;
	/** User-supplied name — several servers of one kind are allowed. */
	label: string;
	enabled: boolean;
	url: string;
	has_token: boolean;
	/** The remote account the credentials resolve to; playback is filtered to it. */
	remote_account: string | null;
	/** Navidrome only — the login it's configured with. */
	username?: string | null;
	poll_interval_secs: number | null;
	libraries_allow: string[];
	libraries_block: string[];
	devices_allow: string[];
	devices_block: string[];
	last_polled_at: number | null;
	last_error: string | null;
	updated_at: number;
}

/** Partial update. Omitting `token` keeps the stored one. */
export type UserSourceUpdate = Partial<
	Omit<UserSource, 'id' | 'kind' | 'has_token' | 'remote_account' | 'last_polled_at' | 'last_error' | 'updated_at'>
> & {
	kind?: string; token?: string; username?: string; password?: string; api_key?: string;
	/** Plex only. Which account on the server to scrobble. Omit for "whoever
	 *  the token belongs to" — the right default for a personal token, wrong
	 *  on a household server where one admin holds it. */
	plex_account?: string;
};

// ── Liner notes (Phase 7) ──

/** Something a note or a love can point at. */
export interface Entity {
	id: number;
	kind: 'recording' | 'release' | 'artist';
	name: string;
	/** Empty for an artist, where the name *is* the artist. */
	artist_name: string;
	mbid: string | null;
	created_at: number;
}

export interface NoteListen {
	id: number;
	title: string;
	artist: string;
	album: string | null;
	timestamp: number;
}

/** Stored from the start because Phase 6 needs it — nothing is served to other
 *  users yet, so today every value behaves as private. */
export type Visibility = 'private' | 'patched' | 'public';

/**
 * How the API addresses a note's target. Use exactly one form: an existing
 * entity, a listen, the track/album/artist *of* a listen, or a spelled-out
 * name. `kind` accepts the shortcode spelling (track/album/artist).
 */
export interface NoteTarget {
	entity_id?: number;
	scrobble_id?: number;
	from_scrobble?: number;
	kind?: 'track' | 'album' | 'artist';
	name?: string;
	artist?: string;
	mbid?: string;
	/** A week, a month or a year of listening. Both of these or neither — the
	 *  key alone is ambiguous, since Reports names periods per unit. */
	period_unit?: 'week' | 'month' | 'year';
	/** Exactly as Reports names it: `2026`, `2026-08`, `2026-W32`. */
	period_key?: string;
	/** A **pin**: one numbered line of a lyric, alongside `artist` and `name`
	 *  (the track's title). Named outright rather than read against anything,
	 *  because a pin has no other subject — it *is* the target. */
	line?: number;
}

/** The line a pin hangs off. `text` is the words when this instance still
 *  holds them and absent when it does not — a pin outlives the lyric it points
 *  at, which is not an error. */
export interface NoteLine {
	artist: string;
	title: string;
	line: number;
	text: string | null;
}

/** The period a note is about, echoed back split rather than as the stored
 *  `month:2026-08` — naming it needs to know which of the three units it is. */
export interface NotePeriod {
	unit: string;
	key: string;
}

export interface Note {
	id: number;
	body_md: string;
	/** Server-rendered and sanitised — safe for {@html}. Absent in list views. */
	body_html?: string;
	excerpt: string;
	visibility: Visibility;
	created_at: number;
	updated_at: number;
	entity: Entity | null;
	listen: NoteListen | null;
	/** Set when the note is about a report period rather than a record or a
	 *  listen. Exactly one of `entity` / `listen` / `period` is ever present. */
	period: NotePeriod | null;
	/** Set when the note is a **pin** — one whose target *is* a lyric line. */
	line: NoteLine | null;
	/** How many distinct lines this note's prose quotes. Counted from the body
	 *  on read, so it cannot drift from what is actually written. */
	lines_quoted: number;
	/** The pins that roll up under this note as footnotes — the design's third
	 *  rendering. A record's note gathers the pins on every track of it that has
	 *  been played; a track's note gathers its own. Present only on a single
	 *  note, not in a listing. */
	pins?: PinFootnote[];
	loved: boolean;
	revisions: number;
}

/** One line of a lyric as the reader draws it.
 *
 *  `n` is 1-based **and blank lines are numbered**: a stanza break is a line,
 *  because a reference is an index and skipping empties would renumber every
 *  line after one — silently re-pointing every quote already written. */
/** One line in Latin letters, as words of parts — a part is one Hangul
 *  block, one Japanese or Chinese token, or one Indic word, with its
 *  original (`o`) and romanization (`r`). `lit` parts are punctuation or text
 *  that was Latin already; `syl` words are sung syllable by syllable (Korean)
 *  and draw a dot between their parts. */
export interface RomanLine {
	text: string;
	words: { parts: { o: string; r: string; lit?: boolean }[]; syl?: boolean }[];
}

/** The original a romanized line was made from — the held words, or the
 *  original-script companion's line when one is used. */
export function romanOriginal(r: RomanLine): string {
	return r.words.map((w) => w.parts.map((p) => p.o).join('')).join(' ');
}

export interface LyricLine {
	n: number;
	at: number | null;
	text: string;
	/** The line in Latin letters, when the lyric is romanized and the line
	 *  needs it. */
	roman?: RomanLine | null;
	/** How many of this user's notes quote this line. */
	quoted?: number;
	/** The note pinned to this line, if there is one. */
	pin?: LinePin | null;
}

export interface LinePin {
	note_id: number;
	body_md: string;
	excerpt: string;
	visibility: Visibility;
	updated_at: number;
}

/** What the reader is given for one track: the words, and the furniture round
 *  them. Everything but `lines` is about the *track*, so it is there whether or
 *  not the lyric turns out to be held. */
export interface LyricDoc {
	artist: string;
	title: string;
	album?: string | null;
	plays?: number;
	language?: string | null;
	timed?: boolean;
	instrumental?: boolean;
	/** These words were corrected here and a later fetch will not overwrite. */
	edited?: boolean;
	/** An LRCLIB record picked from *Other versions* — kept like a correction,
	 *  but still LRCLIB's words. */
	chosen?: boolean;
	/** The LRCLIB record these words are, when they are one. */
	lrclib_id?: number | null;
	/** Present when the lyric is romanized; lines then carry `roman`. */
	romanization?: {
		language: string | null;
		indic: 'readable' | 'iso';
		macrons: boolean;
		/** Made from the original-script companion, not the words shown. */
		from_companion: boolean;
		/** This build reads kanji and hanzi. */
		kanji: boolean;
	} | null;
	/** An LRCLIB record kept beside these words in their original script. */
	companion?: { lrclib_id: number | null; language: string | null; lines_up: boolean } | null;
	/** The stored text, for correcting it by hand. */
	plain?: string | null;
	synced?: string | null;
	lines: LyricLine[];
	tracklist?: LyricSibling[];
	source?: string;
}

/** One LRCLIB record for a track, from `lyricMatches`. */
export interface LyricMatch {
	id: number | null;
	title: string | null;
	artist: string | null;
	/** The release it was filed under. */
	album: string | null;
	duration: number | null;
	synced: boolean;
	lines: number;
	/** Read off the record's script; null for Latin text. */
	language: string | null;
	/** The record already held for this track. */
	held: boolean;
	plain: string | null;
	synced_text: string | null;
}

/** The reader's whole state for whatever is on the deck.
 *
 *  `playing: false` is a normal answer — nothing is on, the theatre says so and
 *  waits. When something is on, this is a `LyricDoc` with the playhead
 *  alongside it, so a cold open lands on the right line without waiting for the
 *  first deck poll. */
export type LiveLyricDoc =
	| { playing: false }
	| (LyricDoc & {
			playing: true;
			/** Seconds in, as of the moment the server answered. */
			position: number;
			/** Whether that is a measurement. A guess must not drive the
			 *  highlight — the same distinction the deck itself draws. */
			position_known: boolean;
			paused: boolean;
			duration: number | null;
	  });

/** One other track of the same record, for the reader's sidebar. */
export interface LyricSibling {
	title: string;
	plays: number;
	duration: number | null;
	track_number: number | null;
	/** What is held for it. Absent for a track nobody has asked about, which is
	 *  a different thing from one that has no words — the sidebar dims the
	 *  first and marks the second. */
	lyrics?: 'words' | 'instrumental';
}

/** A pin as it reads under the note it belongs to. */
export interface PinFootnote {
	note_id: number;
	line: NoteLine;
	body_md: string;
	excerpt: string;
	updated_at: number;
}

/** A pinned line in "Lines you've written about". */
export interface PinnedLine {
	note_id: number;
	line: NoteLine;
	excerpt: string;
	updated_at: number;
}

/** The key `/api/v1/lyrics/keys` returns, derived for a row in hand.
 *
 *  **Must stay in step with `LyricsClient::track_key`** — artist and title,
 *  each trimmed and lowercased, joined by a pipe. If the two drift, "Read the
 *  words" silently stops appearing on rows that have lyrics and nothing errors.
 *  The same standing trap as `loveKey`, which folds the artist half the same
 *  way. */
export function lyricKey(artist: string, title: string): string {
	return `${artist.trim().toLowerCase()}|${title.trim().toLowerCase()}`;
}

/** Per-target state around one listen. `entity_id` is null until something has
 *  actually been written or loved — inspecting a listen creates nothing. */
export interface Annotation {
	name: string;
	entity_id: number | null;
	note_id: number | null;
	loved: boolean;
}

export interface ListenAnnotations {
	listen_note_id: number | null;
	track: Annotation | null;
	album: Annotation | null;
	artist: Annotation | null;
}

/** How the notes list is narrowed. `kind` is the stored spelling, not the
 *  shortcode one, because that is what the server filters on. */
export interface NoteQuery {
	q?: string;
	kind?: '' | 'recording' | 'release' | 'artist' | 'listen' | 'period' | 'line';
	sort?: 'updated' | 'created' | 'title';
}

export interface NoteRevision {
	id: number;
	body_md: string;
	excerpt: string;
	created_at: number;
}

export interface PlexAccount {
	id: number;
	name: string;
}

export interface SourceTestResult {
	ok: boolean;
	libraries?: string[];
	account?: string | null;
	account_error?: string | null;
	/** Every account on the server, for the "scrobble as" picker. Empty when
	 *  the credential may not list them — the picker just isn't offered.
	 *  Jellyfin needs this more than Plex: a dashboard API key belongs to no
	 *  user, so the server cannot name an account on its own. */
	accounts?: PlexAccount[];
	error?: string;
}

export interface HealthStatus {
	status: string;
	service: string;
	version: string;
}

export interface AuthStatus {
	needs_setup: boolean;
	authenticated: boolean;
	is_admin: boolean;
}

export interface User {
	id: number;
	username: string;
	display_name: string | null;
	role: string;
	created_at: number;
	disabled: boolean;
}

export interface UserInfo {
	user_id: number;
	username: string;
	is_admin: boolean;
}

/** Raw quality columns, carried so `qualityLabel()` produces the same tag it
 *  does on a history row rather than a second implementation. */
export interface TrackAggregate {
	title: string;
	plays: number;
	duration: number | null;
	track_number: number | null;
	album: string | null;
	format_type: string | null;
	codec: string | null;
	bit_depth: number | null;
	sample_rate: number | null;
	dsd_multiplier: number | null;
	delivery_codec: string | null;
	is_lossless: boolean | null;
}

export interface AlbumAggregate {
	name: string;
	plays: number;
	year: number | null;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
}

/** Pre-formatted by the server so every surface phrases a number identically. */
export interface StatCard {
	k: string;
	v: string;
	sub: string;
	tone: 'plain' | 'dark';
}

/** Prose from the description chain. `text` null means every provider was
 *  asked and none had anything — a tombstone, not a pending fetch.
 *  `attribution` is **mandatory to display** when present (Last.fm is CC BY-SA). */
export interface BioBlock {
	text: string | null;
	attribution: string | null;
	provider: string | null;
	url: string | null;
}

export interface AlbumView {
	id: number;
	title: string;
	artist: string;
	artist_entity_id: number | null;
	mbid: string | null;
	year: number | null;
	track_count: number;
	/** Time you've spent on this record — not the record's length. */
	listening_seconds: number;
	plays: number;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
	/** True when the cover was set by hand, in which case it replaces the one
	 *  derived from the listens rather than sitting in front of it. */
	cover_override: boolean;
	loved: boolean;
	bio: BioBlock;
	stats: StatCard[];
	tracks: TrackAggregate[];
	more: AlbumAggregate[];
}

/** One track of a record's own tracklist, as the lookup reports it. */
export interface TracklistTrack {
	/** "A", "B", … A CD comes back as a single side. */
	side: string;
	position: number;
	title: string;
	/** Set where the release credits the track to somebody other than the
	 *  record's artist — a compilation, or a guest. */
	artist: string | null;
	duration_secs: number | null;
}

/**
 * A record's own tracklist, pulled from MusicBrainz on request.
 *
 * Deliberately not part of `AlbumView`: it costs a rate-limited call to an
 * external service, and it is never stored — the page matches it against the
 * plays it already holds, because writing a tracklist into the history would be
 * inventing listens that never happened.
 */
export interface AlbumTracklist {
	source: string;
	mbid_release: string | null;
	title: string;
	artist: string;
	year: number | null;
	format: string | null;
	tracks: TracklistTrack[];
}

export interface ArtistView {
	id: number;
	name: string;
	mbid: string | null;
	/** Portrait: from the metadata chain, from the user's own library through
	 *  the proxy, or set by hand. Null means the lettered tile. */
	image_url: string | null;
	/** The wide one, for tiles that give an artist the whole frame — the
	 *  Reports chapter card. A different photograph from the portrait, never a
	 *  crop of it, and only ever set by hand. */
	landscape_url: string | null;
	/** The `artists` row this name maps to — keyed by name and shared, unlike
	 *  `id` which is this user's entity. Addresses the portrait proxy and the
	 *  manual override. */
	artist_row_id: number | null;
	rank: number;
	artist_total: number;
	plays: number;
	album_count: number;
	track_count: number;
	loved: boolean;
	bio: BioBlock;
	stats: StatCard[];
	albums: AlbumAggregate[];
	top_tracks: TrackAggregate[];
	trend: { label: string; value: number }[];
}

export interface ChartRow {
	name: string;
	/** Secondary line — the artist, for albums and tracks. */
	sub: string | null;
	/** Tracks only: the record it's on, so the row can open the album. */
	album: string | null;
	plays: number;
	seconds: number;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
}

export type ChartKind = 'artists' | 'albums' | 'tracks';
export type ChartRange = 'today' | 'd7' | 'month' | 'year' | 'all';

export interface Session {
	id: number;
	/** Stable across merges and splits; the id a shared session will carry. */
	uuid: string | null;
	title: string | null;
	/** A note about the sitting itself — distinct from a liner note, which is
	 *  about a record and hangs off an entity or a single listen. */
	notes: string | null;
	started_at: number;
	ended_at: number | null;
	track_count: number;
	total_duration: number;
	avg_quality_score: number | null;
	listening_context: string | null;
	chain_id: number | null;
	chain_name: string | null;
	/** Read off the sitting's listens rather than stored on the session, so it
	 *  cannot disagree with them. */
	chain_variant_id: number | null;
	chain_variant_name: string | null;
	device_id: number | null;
	device_name: string | null;
	token_id: number | null;
	token_name: string | null;
}

export interface ReleaseCandidate { mbid: string; plays: number }

export interface MixedAlbum {
	artist: string;
	album: string;
	plays: number;
	variants: number;
	/** The plurality — a proposal, not a verdict. */
	winner: string | null;
	winner_plays: number;
	/** Listens with no release at all; unifying stamps these too. */
	untagged: number;
	/** The whole ballot, most-claimed first, so a weak plurality is visible. */
	candidates: ReleaseCandidate[];
}

/** One track on a physical side. */
export interface DurationProposal {
	side: string;
	position: number;
	title: string;
	/** What the shelf holds now — null is the case this exists for. */
	current: number | null;
	/** What the looked-up pressing says. Null where nothing matched, never a guess. */
	proposed: number | null;
}

export interface DurationPreview {
	/** Every track already has a duration; there is nothing to do. */
	complete: boolean;
	/** The lookup could not be made — busy, throttled or too slow. Distinct from
	 *  an empty result, which means it was asked and there is nothing. */
	unavailable?: boolean;
	source: { title: string; artist: string; year?: number | null; format?: string | null; country?: string | null } | null;
	durations: DurationProposal[];
	note?: string;
}

export interface PhysicalTrack {
	side: string;
	position: number;
	title: string;
	artist?: string | null;
	duration_secs?: number | null;
	/**
	 * Which physical disc of the set this is on, from 1. Absent for anything
	 * catalogued before Tapedeck kept it, and for Discogs lookups, whose
	 * tracklist is flat and names no medium — so it means "not recorded",
	 * never "disc 1". Anything sending a tracklist back must carry it through,
	 * or re-saving a box set silently forgets its shape.
	 */
	disc?: number | null;
}

/** A candidate pressing from MusicBrainz or Discogs. */
export interface ReleaseCandidate {
	source: 'musicbrainz' | 'discogs';
	artist: string;
	title: string;
	year: number | null;
	country: string | null;
	format: string | null;
	mbid_release: string | null;
	discogs_id: string | null;
	barcode: string | null;
	catalog_number: string | null;
	artwork_url: string | null;
	tracks: PhysicalTrack[];
}

/** One scan of a pressing — a sleeve, a label, a booklet page.
 *
 *  `src` and `thumb` are built server-side so a stored file path can never
 *  reach the page: a provider's image is its own URL, an uploaded scan is a
 *  Tapedeck URL that checks who is asking. */
export interface PhysicalImage {
	id: number;
	release_id: number;
	/** `front` | `back` | `booklet` | `medium` | `spine` | … Free text, because
	 *  the Cover Art Archive's vocabulary grows and a scan of something it has
	 *  no word for is still a scan. */
	kind: string;
	provider: 'caa' | 'discogs' | 'upload';
	src: string;
	thumb: string;
	/** The archive editor's own note — "page 3 of 20", "inner left panel".
	 *  Without it nineteen booklet pages are indistinguishable. */
	caption: string | null;
	position: number;
	bytes: number | null;
	mime: string | null;
	added_at: number;
}

/** What one side of a pressing has been through.
 *
 *  Derived from the sides played, never accumulated — the same rule the
 *  release's own totals and every gear hour follow.
 *
 *  Only sides that have actually been played come back. A side nobody has
 *  turned over is absent rather than a row of zeros, because the tracklist is
 *  what knows a side exists; the page fills those in. */
export interface SidePlays {
	/** As recorded — "A", "B", or a disc number. */
	side: string;
	plays: number;
	/** Running time of what actually played, summed. Nominal, not wall clock. */
	secs: number;
	last_played: number | null;
}

/** A record or tape on the shelf. Counters are derived, never accumulated. */
/** The shelf's formats. Mirrors the CHECK on `physical_releases.kind`. */
export type ShelfKind = 'vinyl' | 'cassette' | 'cd' | 'sacd';

/** The side currently on the deck, held server-side.
 *
 *  The countdown runs in the page, but this is what makes it recoverable: open
 *  the shelf on another device, or after a crash, and the side is still on. */
export interface Spin {
	release_id: number;
	kind: ShelfKind;
	artist: string;
	title: string;
	artwork_url: string | null;
	mbid_release: string | null;
	side: string;
	started_at: number;
	/** Null while running. */
	paused_at: number | null;
	/** Seconds spent stopped, so elapsed is wall clock minus this. */
	paused_total: number;
	outcomes: { position: number; started_at: number; skipped: boolean }[];
	chain_id: number | null;
	equipment_id: number | null;
	/** How far into the side the needle went down. */
	start_secs: number;
	updated_at: number;
}

export interface SpinUpdate {
	release_id: number;
	side: string;
	started_at: number;
	paused_at?: number | null;
	paused_total?: number;
	outcomes?: { position: number; started_at: number; skipped: boolean }[];
	chain_id?: number | null;
	equipment_id?: number | null;
	start_secs?: number;
}

export interface PhysicalRelease {
	id: number;
	kind: 'vinyl' | 'cassette' | 'cd' | 'sacd';
	artist: string;
	title: string;
	year: number | null;
	mbid_release: string | null;
	discogs_id: string | null;
	barcode: string | null;
	catalog_number: string | null;
	artwork_url: string | null;
	/** JSON-encoded PhysicalTrack[] — parsed by the page that shows it. */
	tracks: string;
	notes: string | null;
	added_at: number;
	/** When you bought it. Null for anything shelved before the field existed,
	 *  and for anything you would only be guessing at. */
	purchased_at: number | null;
	/** How it came to be yours — `new`, `used`, `gift`, `inherited`. Null means
	 *  nobody said, never "new": a shelf catalogued before the field existed
	 *  must not be retroactively claimed as bought new. */
	acquired_as: string | null;
	/** Where from — the shop, the fair, the person. Free text, because that is
	 *  the half that makes a thrift find a story rather than a flag. */
	acquired_from: string | null;
	/** What it is normally played on. The deck pre-fills from these and writes
	 *  the answer back, so a record with one turntable is asked once. */
	chain_id: number | null;
	equipment_id: number | null;
	/** Where a tape was left. Cassettes only — a record has random access and no
	 *  state to remember, so these stay null for every other medium. */
	parked_side: string | null;
	parked_secs: number | null;
	play_count: number;
	played_secs: number;
	last_played: number | null;
	image_count: number;
	/** The front sleeve out of this pressing's own scans. Prefer it over
	 *  `artwork_url`, which is only whatever the barcode lookup happened to
	 *  return and is empty for a great many releases. */
	cover_thumb: string | null;
}

export interface PlayResult {
	listens_added: number;
	/** Stored, but excluded from every count and never forwarded. */
	skipped?: number;
	tracks: number;
	duration_secs: number;
	started_at: number;
	/** Which side to reach for next — wraps, and knows a double LP has four. */
	next_side: string | null;
}

/** One suggestion. Same shape whichever list produced it — the reason lives on
 *  the list, not the row. */
export interface RediscoveryRow {
	name: string;
	artist: string;
	album: string | null;
	plays: number;
	last_played: number | null;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
}

/** A card, not a bare list: the accent, blurb and stat are what make a
 *  collection legible at a glance. No play/queue action — Tapedeck has no
 *  playback, and a dead button is worse than an absent one. */
export interface RediscoveryCollection {
	key: string;
	title: string;
	blurb: string;
	/** Per-collection accent, straight from the design. */
	accent: string;
	/** Names the evidence ("Last played 14 months ago"), not the blurb again. */
	stat: string;
	count: number;
	rows: RediscoveryRow[];
}

export interface Rediscovery {
	quiet_days: number;
	window: string;
	/** Whichever collection has the most to say — a fixed one would leave a new
	 *  instance with an empty hero. */
	featured: RediscoveryCollection | null;
	collections: RediscoveryCollection[];
	gear: { id: number; name: string; type: string }[];
}

/** One axis of the Playlist Lab's space.
 *
 *  `group` matters: `sound` axes come from the Every Noise genre map (reached
 *  through the track's artist, so every track by one artist shares them),
 *  `habit` axes come from your own listening. Mixing them is deliberate and the
 *  weighting slider is what keeps "similar" from meaning "played at the same
 *  hour". */
export interface LabDim {
	key: string;
	label: string;
	low: string;
	high: string;
	group: 'sound' | 'habit';
}

export interface LabTrack {
	title: string;
	artist: string;
	album: string | null;
	plays: number;
	vector: number[];
}

export interface PlaylistLab {
	dims: LabDim[];
	tracks: LabTrack[];
	/** Indices into `tracks` that you have loved. */
	loved: number[];
	pool: number;
	considered: number;
	/** Tracks dropped because no genre of theirs has a place on the map. */
	unplaced: number;
	min_plays: number;
	/** Always "artist" today — the honest ceiling, stated by the server. */
	granularity: string;
	note: string;
}

/** A server this instance could send a playlist to.
 *
 *  Comes from the user's own enabled sources — there is no separate credential
 *  for pushing, so the options are exactly the servers they already set up. */
export interface PlaylistTarget {
	source_id: number;
	kind: string;
	/** "Plex", "Jellyfin", "Subsonic" — the protocol, which is why a Navidrome
	 *  source reads as Subsonic: any OpenSubsonic server takes the same calls. */
	protocol: string;
	label: string;
	account: string | null;
}

/** Names only. Tapedeck stores no server item ids for a generated list, so the
 *  far end has to be searched — which is why a push can come back partial. */
export interface PlaylistPushTrack {
	title: string;
	artist: string;
	album?: string | null;
	/** Which kind of miss it was. A server that returned nothing has a search
	 *  problem; one that returned the right song and had it refused has a
	 *  matching problem, and they need different things from the user. */
	reason?: 'search_failed' | 'no_candidates' | 'artist_mismatch' | 'title_mismatch';
	/** What the far end offered instead, each with its own id — so a candidate
	 *  can be *chosen* rather than only read. */
	offered?: { id: string; label: string }[];
}

export interface PlaylistPushResult {
	playlist_id: string;
	name: string;
	protocol: string;
	target: PlaylistTarget;
	requested: number;
	matched: number;
	/** Named rather than counted: "nine of twelve" without saying which three is
	 *  a number that is just wrong to anyone trying to fix it. */
	missing: PlaylistPushTrack[];
	/** How many tracks were answered from the resolution cache rather than by
	 *  asking the far end. On a re-push this should be nearly everything. */
	from_cache?: number;
	/** Whether this rewrote the playlist a previous push made, or made a new
	 *  one — different things to say to somebody looking at their library. */
	updated_existing?: boolean;
}

export interface SanitizePreview {
	/** Albums whose listens disagree about which release they are. */
	mixed_albums: MixedAlbum[];
	/** `Artist feat. X` strings that fragment an album or artist page. */
	featured_artists: { artist: string; primary: string; plays: number }[];
	/** One artist spelled several ways ("A.R. Rahman" / "A. R. Rahman"). */
	artist_variants: {
		primary: string;
		primary_plays: number;
		variants: [string, number][];
		/** Differs only in capitalisation. Cosmetic — every counting query
		 *  already lowercases, so these display inconsistently but count
		 *  correctly. Anything else genuinely splits the play total. */
		case_only: boolean;
	}[];
	/** One track on a record credited two different ways — the film-soundtrack
	 *  case, where the album carries the music director and each track its
	 *  singers. `primary_album_tracks` is *why* the winner won and shows how
	 *  strong the case is: a director on twelve tracks is obvious, two is a guess. */
	split_attributions: SplitAttribution[];
}

export interface SplitAttribution {
	title: string;
	album: string;
	primary: string;
	primary_plays: number;
	primary_album_plays: number;
	primary_album_tracks: number;
	folding: [string, number][];
}

export interface SanitizeResult {
	splits_merged?: number;
	split_listens_changed?: number;
	albums_fixed: number;
	album_listens_changed: number;
	artists_folded: number;
	artist_listens_changed: number;
	variants_merged: number;
	variant_listens_changed: number;
}

export interface SearchResults {
	artists: { name: string; plays: number }[];
	albums: { name: string; artist: string; plays: number }[];
	tracks: { title: string; artist: string; album: string | null; plays: number }[];
}

export interface TokenInfo {
	id: number;
	name: string;
	scopes: string;
	created_at: number;
	last_used_at: number | null;
	/** Listens submitted with this token attribute here unless the listen
	 *  itself says otherwise. One token per app is the intended use. */
	default_chain_id: number | null;
	default_chain_name: string | null;
	/** Whether this client's `listen_type: "import"` submissions are live
	 *  listens, and so are forwarded on to Last.fm / ListenBrainz rather than
	 *  stored only. Some players (fooyin) send `import` for every scrobble. */
	imports_are_live: boolean;
}

export interface RatingSample {
	kind: string;
	artist: string;
	name: string;
	album: string | null;
	source_kind: string;
	/** The number the server gave, verbatim. */
	rating: number | null;
	/** What that number was out of. Meaningless apart from `rating`. */
	scale_max: number;
	/** The same rating on the display scale. Presentation only. */
	shown: number;
	starred: boolean;
	loved: boolean;
}

export interface RatingSourceSummary {
	source_id: number;
	source_kind: string;
	label: string;
	rated: number;
	starred: number;
	scale_max: number;
	fetched_at: number | null;
}

export interface RatingSourcePull {
	source_id: number;
	kind: string;
	label: string;
	found: number;
	error: string | null;
}

export interface RatingsView {
	scale: string;
	scale_label: string;
	scale_choices: number[];
	threshold: number;
	include_starred: boolean;
	sources: RatingSourceSummary[];
	sample: RatingSample[];
}

export interface RatingApplyResult {
	threshold: number;
	scale: string;
	include_starred: boolean;
	report: {
		qualifying: number;
		matched: number;
		unmatched: number;
		loved: number;
		withdrawn: number;
		already: number;
		dry_run: boolean;
	};
}

class TapedeckAPI {
	private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
		const opts: RequestInit = {
			method,
			headers: { 'Content-Type': 'application/json' },
			credentials: 'same-origin',
		};
		if (body) opts.body = JSON.stringify(body);
		const res = await fetch(`${BASE}${path}`, opts);
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}

	private async get<T>(path: string): Promise<T> {
		return this.request('GET', path);
	}

	private async post<T>(path: string, body?: unknown): Promise<T> {
		return this.request('POST', path, body);
	}

	private async del<T>(path: string): Promise<T> {
		return this.request('DELETE', path);
	}

	private async patch<T>(path: string, body?: unknown): Promise<T> {
		return this.request('PATCH', path, body);
	}

	private async put<T>(path: string, body?: unknown): Promise<T> {
		return this.request('PUT', path, body);
	}

	/** DELETE that tolerates an empty 204 body (the JSON helpers assume a body). */
	private async delNoBody(path: string): Promise<void> {
		const res = await fetch(`${BASE}${path}`, { method: 'DELETE', credentials: 'same-origin' });
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
	}

	// ── Auth ──
	async authStatus(): Promise<AuthStatus> {
		return this.get('/api/v1/auth/status');
	}

	async login(username: string, password: string): Promise<UserInfo> {
		return this.post('/api/v1/auth/login', { username, password });
	}

	async logout(): Promise<void> {
		await this.post('/api/v1/auth/logout');
	}

	async setup(username: string, password: string, display_name?: string): Promise<{ user_id: number; token: string; message: string }> {
		return this.post('/api/v1/auth/setup', { username, password, display_name });
	}

	async me(): Promise<UserInfo> {
		return this.get('/api/v1/auth/me');
	}

	// ── Health ──
	async health(): Promise<HealthStatus> {
		return this.get('/health');
	}

	// ── Now playing ──
	async getNowPlaying(): Promise<NowPlayingState> {
		return this.get('/api/v1/now-playing');
	}

	// ── Sources (per user) ──
	async getSources(): Promise<{ sources: UserSource[]; kinds: string[] }> {
		return this.get('/api/v1/sources');
	}

	async createSource(body: UserSourceUpdate): Promise<UserSource> {
		return this.post('/api/v1/sources', body);
	}

	async updateSource(id: number, body: UserSourceUpdate): Promise<UserSource> {
		return this.request('PUT', `/api/v1/sources/${id}`, body);
	}

	async deleteSource(id: number): Promise<void> {
		return this.delNoBody(`/api/v1/sources/${id}`);
	}

	/** Pass `kind` to test before saving, or `id` to re-test a stored source. */
	async testSource(body: {
		kind?: string; id?: number; url?: string;
		token?: string; username?: string; password?: string; api_key?: string;
	}): Promise<SourceTestResult> {
		return this.post('/api/v1/sources/test', body);
	}

	// ── Scrobbles ──
	async getScrobbles(params?: {
		limit?: number; offset?: number; artist?: string;
		album?: string; track?: string; after?: number; before?: number;
		/** true for listening marked as heard in company, false for the rest.
		 *  Omit for everything — an unmarked listen is unmarked, not solitary. */
		social?: boolean;
	}): Promise<{ scrobbles: Scrobble[]; count: number }> {
		const query = new URLSearchParams();
		if (params?.limit) query.set('limit', String(params.limit));
		if (params?.offset) query.set('offset', String(params.offset));
		if (params?.artist) query.set('artist', params.artist);
		if (params?.album) query.set('album', params.album);
		if (params?.track) query.set('track', params.track);
		if (params?.after) query.set('after', String(params.after));
		if (params?.before) query.set('before', String(params.before));
		// Explicitly against undefined: `social: false` is a real filter and a
		// truthiness test would drop it.
		if (params?.social !== undefined) query.set('social', String(params.social));
		const qs = query.toString();
		return this.get(`/api/v1/scrobbles${qs ? '?' + qs : ''}`);
	}

	async getDashboardStats(): Promise<DashboardStats> {
		return this.get('/api/v1/stats/dashboard');
	}

	/** `metric` decides how the three top lists are *ranked*, not only what
	 *  number is displayed — they are cut to 50 rows server-side, so sorting in
	 *  the browser would only reorder the top 50 by plays. */
	async getRangedStats(range: StatsRange = 'all', metric: 'plays' | 'hours' = 'plays'): Promise<RangedStats> {
		return this.get(`/api/v1/stats?range=${range}&metric=${metric}`);
	}

	/** Period reports, newest first. */
	async getReports(
		unit: 'week' | 'month' | 'year' = 'month',
		limit = 12,
		offset = 0
	): Promise<Reports> {
		return this.get(`/api/v1/reports?unit=${unit}&limit=${limit}&offset=${offset}`);
	}

	async getPersonality(range: StatsRange = 'all'): Promise<Personality> {
		return this.get(`/api/v1/personality?range=${range}`);
	}

	/** How much of the history MusicBrainz has identified at recording level.
	 *  `unmatched_tracks` are counted in `pending` and will never resolve —
	 *  reported separately so a bar that stops short reads as an answer rather
	 *  than a stall. */
	async getMbidStatus(): Promise<{
		total: number;
		resolved: number;
		pending: number;
		unmatched_tracks: number;
	}> {
		return this.get('/api/v1/mbids/status');
	}
	async backfillMbids(): Promise<{ status: string }> {
		return this.post('/api/v1/mbids/backfill');
	}

	/** How far the lyric pass has got.
	 *
	 *  There is deliberately no companion `backfill` call: lyrics are fetched
	 *  by the engine tick alone, at LRCLIB's asked-for pace, so a button here
	 *  would drive an unbounded number of requests at a free service. This is
	 *  a readout.
	 *
	 *  `asked` and `unresolved` do **not** share a population and must never be
	 *  divided into one another — `unresolved` counts tracks still showing
	 *  `English/Other`, which includes tracks already asked about whose lyric
	 *  turned out to be Latin. Report them as two counts, never as a bar. */
	/** The other records LRCLIB holds for a track.
	 *
	 *  Not versions — LRCLIB has none. These are different *records* for the
	 *  same song, which is what the design's version picker was really reaching
	 *  for: the way to fix a wrong match without typing the words out.
	 *
	 *  Reaches the network, so it is called on a press and never on a render.
	 *  A rate limit comes back as `unavailable` with a reason rather than an
	 *  error, because LRCLIB asking us to wait is not a failure of ours. */
	async lyricMatches(
		artist: string,
		title: string,
		search?: { artist?: string; title?: string }
	): Promise<{
		matches: LyricMatch[];
		unavailable?: boolean;
		why?: string;
	}> {
		const q = new URLSearchParams({ artist, title });
		// Names to search under when the tags differ from LRCLIB's. The track
		// stays `artist`/`title`, which is what `held` is worked out against.
		if (search?.artist?.trim()) q.set('search_artist', search.artist.trim());
		if (search?.title?.trim()) q.set('search_title', search.title.trim());
		return this.get(`/api/v1/lyrics/matches?${q}`);
	}

	/** Hold an LRCLIB record picked from *Other versions*, by its id.
	 *
	 *  The server fetches the record itself and credits it to LRCLIB. Never
	 *  `fixLyrics` for this: that is the correction path and would credit a
	 *  stranger's upload to "You". */
	async adoptLyrics(body: {
		artist: string;
		title: string;
		id: number;
		/** Keep it beside the words as their original script instead. */
		original_script?: boolean;
	}): Promise<
		| { unavailable: true; why: string }
		| { status: 'companion'; language: string | null; lines: number }
		| {
				status: 'adopted';
				language: string | null;
				lines_before: number;
				lines_after: number;
				renumbered: boolean;
		  }
	> {
		return this.post('/api/v1/lyrics/adopt', body);
	}

	/** Forget the original-script companion kept beside a track's words. */
	async clearCompanion(artist: string, title: string): Promise<{ status: 'cleared' }> {
		const q = new URLSearchParams({ artist, title });
		return this.del(`/api/v1/lyrics/companion?${q}`);
	}

	async getLyricsStatus(): Promise<{
		available: boolean;
		why?: string;
		asked?: number;
		with_lyrics?: number;
		instrumental?: number;
		unresolved: number;
	}> {
		return this.get('/api/v1/lyrics/status');
	}

	async getArtworkStatus(): Promise<{ total: number; resolved: number; pending: number }> {
		return this.get('/api/v1/artwork/status');
	}
	async backfillArtwork(): Promise<{ status: string }> {
		return this.post('/api/v1/artwork/backfill');
	}

	async getLanguageStatus(): Promise<{ total: number; resolved: number; pending: number }> {
		return this.get('/api/v1/languages/status');
	}
	async backfillLanguages(): Promise<{ status: string }> {
		return this.post('/api/v1/languages/backfill');
	}

	async patchScrobble(id: number, changes: Partial<{
		title: string; artist: string; album: string; mbid_recording: string;
		signal_chain_id: number; listening_context: string; format_type: string; codec: string;
	}>): Promise<Scrobble> {
		return this.patch(`/api/v1/scrobbles/${id}`, changes);
	}

	async deleteScrobble(id: number): Promise<void> {
		return this.delNoBody(`/api/v1/scrobbles/${id}`);
	}

	async clearScrobbles(): Promise<void> {
		return this.delNoBody('/api/v1/scrobbles?confirm=true');
	}

	// ── Preferences ──
	async getPreferences(): Promise<Record<string, unknown>> { return this.get('/api/v1/preferences'); }
	async putPreferences(patch: Record<string, unknown>): Promise<Record<string, unknown>> {
		return this.request('PUT', '/api/v1/preferences', patch);
	}

	// ── Export (returns a Blob to trigger a download) ──
	async exportBlob(format: 'json' | 'csv'): Promise<Blob> {
		const res = await fetch(`${BASE}/api/v1/export?format=${format}`, { credentials: 'same-origin' });
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.blob();
	}

	async backupBlob(): Promise<Blob> {
		const res = await fetch(`${BASE}/api/v1/backup`, { credentials: 'same-origin' });
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.blob();
	}

	// ── Per-user scrobble connections ──
	async getConnections(): Promise<{ connections: Connection[]; lastfm_available: boolean }> {
		return this.get('/api/v1/connections');
	}
	async connectListenBrainz(token: string): Promise<{ connected: boolean; username: string | null }> {
		return this.request('PUT', '/api/v1/connections/listenbrainz', { token });
	}
	async disconnect(service: string): Promise<void> {
		return this.delNoBody(`/api/v1/connections/${service}`);
	}
	/** `service` is 'lastfm' or 'librefm' — both speak the AudioScrobbler auth flow. */
	async scrobblerStart(service: 'lastfm' | 'librefm'): Promise<{ token: string; auth_url: string }> {
		return this.post(`/api/v1/connections/${service}/start`);
	}
	async scrobblerComplete(
		service: 'lastfm' | 'librefm',
		token: string
	): Promise<{ connected: boolean; username: string | null }> {
		return this.post(`/api/v1/connections/${service}/complete`, { token });
	}

	/**
	 * TIDAL is connected over OAuth rather than with a pasted token, so this is
	 * two steps: ask for the URL, open it, and the server's callback finishes
	 * the job. `configured` is about the instance (has the operator registered
	 * an app?), `connected` about this user.
	 */
	async tidalStatus(): Promise<{ configured: boolean; connected: boolean }> {
		return this.get('/api/v1/tidal/status');
	}
	async tidalConnect(): Promise<{ authorize_url: string }> {
		return this.post('/api/v1/tidal/connect');
	}
	async tidalDisconnect(): Promise<{ disconnected: boolean }> {
		return this.request('DELETE', '/api/v1/tidal/disconnect');
	}
	/** Admin: the server-wide app registration. Never returns the credential. */
	async tidalSettings(): Promise<{
		configured: boolean;
		stored: boolean;
		has_secret: boolean;
		from_env: boolean;
		redirect_uri: string;
	}> {
		return this.get('/api/v1/tidal/settings');
	}
	// ── Patch: patches, the inbox and the Reel ──
	//
	// Session-only on the server, all of it: a token is held by a scrobble
	// client, and no scrobble client has business approving who may read its
	// owner's listening.
	async getPatchDecks(): Promise<PatchDecks> {
		return this.get('/api/v1/patch/decks');
	}

	async requestPatch(username: string): Promise<{ uuid: string; state: string }> {
		return this.request('POST', '/api/v1/patch', { username });
	}

	async settlePatch(uuid: string, accept: boolean): Promise<{ state: string }> {
		return this.request('POST', `/api/v1/patch/requests/${encodeURIComponent(uuid)}/settle`, {
			accept
		});
	}

	async unpatch(username: string): Promise<{ unpatched: boolean }> {
		return this.request('DELETE', `/api/v1/patch/deck/${encodeURIComponent(username)}`);
	}

	/** The same endpoint the chain drawer calls — one state, two places. */
	async setChainShared(chainId: number, shared: boolean): Promise<{ shared: boolean }> {
		return this.request('PUT', `/api/v1/patch/chains/${chainId}/shared`, { shared });
	}

	/** `days` bounds the window the ranking is computed over — omit for all
	 *  time. Server-side, because the query cuts to the top N by plays and
	 *  narrowing here would rank a week among artists chosen on all-time
	 *  totals. */
	async getPatchArtists(days?: number): Promise<{ artists: PatchArtist[] }> {
		return this.get(`/api/v1/patch/artists${days ? `?days=${days}` : ''}`);
	}

	async getPatchDiscover(): Promise<{ decks: DiscoverDeck[] }> {
		return this.get('/api/v1/patch/discover');
	}

	async getPatchInstance(days = 7): Promise<{ days: number; pulse: InstancePulse }> {
		return this.get(`/api/v1/patch/instance?days=${days}`);
	}

	async getPatchOps(): Promise<{
		ops: FederationOps;
		vocabulary: { verb: string; object: string; namespace: string; frozen: boolean };
		delivery: boolean;
	}> {
		return this.get('/api/v1/patch/ops');
	}

	async getDeck(username: string): Promise<DeckView> {
		return this.get(`/api/v1/patch/deck/${encodeURIComponent(username)}`);
	}

	async dub(body: {
		to: string;
		kind: string;
		artist: string;
		name?: string | null;
		reason?: string | null;
		because: string;
	}): Promise<{ id: number }> {
		return this.request('POST', '/api/v1/patch/dub', body);
	}

	async getDubs(): Promise<{ dubs: Dub[] }> {
		return this.get('/api/v1/patch/dubs');
	}

	/** Whether accepted Shared Spool plays reach this user's scrobble services.
	 *  `window_days` is the freshness bound that applies even when it is on —
	 *  read from the server rather than hardcoded, so a screen cannot promise
	 *  something the relay rule would then refuse. */
	async getSpoolForwarding(): Promise<{ enabled: boolean; window_days: number }> {
		return this.get('/api/v1/connections/spool-forwarding');
	}

	async setSpoolForwarding(enabled: boolean): Promise<{ enabled: boolean }> {
		return this.request('PUT', '/api/v1/connections/spool-forwarding', { enabled });
	}

	async getSpool(): Promise<{ spool: SpoolView | null; captures: SpoolCapture[] }> {
		return this.get('/api/v1/patch/spool');
	}

	async startSpool(body: {
		title?: string | null;
		with: string[];
		guests?: string[];
	}): Promise<{ uuid: string }> {
		return this.request('POST', '/api/v1/patch/spool', body);
	}

	/** Every sitting you were in, newest first.
	 *
	 *  **Membership is the gate, not the patch** — you were in the room, and
	 *  un-patching afterwards has no standing to un-happen an evening you
	 *  attended. What it shows is still gated by the per-chain switch. */
	async getSpoolHistory(limit = 50): Promise<{ sittings: SpoolSummary[] }> {
		return this.get(`/api/v1/patch/spools?limit=${limit}`);
	}

	async getSpoolDetail(uuid: string): Promise<SpoolDetail> {
		return this.get(`/api/v1/patch/spools/${encodeURIComponent(uuid)}`);
	}

	/** Name a sitting, write about it, or say who else was there. The host's.
	 *
	 *  An omitted field is left alone and an explicit null clears it, which is
	 *  why `title` and `notes` are `string | null | undefined`. */
	async editSpool(
		uuid: string,
		body: { title?: string | null; notes?: string | null; guests?: string[] }
	): Promise<{ saved: boolean }> {
		return this.request('PATCH', `/api/v1/patch/spools/${encodeURIComponent(uuid)}`, body);
	}

	/** Add people to a sitting that has already started, or already ended.
	 *
	 *  `present_from` absent means they were here all along and are offered the
	 *  whole evening — the common case by a distance. */
	async addSpoolMembers(
		uuid: string,
		body: { with: string[]; present_from?: number | null }
	): Promise<{ added: number }> {
		return this.request(
			'POST',
			`/api/v1/patch/spools/${encodeURIComponent(uuid)}/members`,
			body
		);
	}

	/** Correct when *you* were in the room. Returns how many plays that has
	 *  just put in your inbox. */
	async setSpoolPresence(
		uuid: string,
		body: { present_from?: number | null; present_until?: number | null }
	): Promise<{ offered: number }> {
		return this.request(
			'PUT',
			`/api/v1/patch/spools/${encodeURIComponent(uuid)}/presence`,
			body
		);
	}

	/** Fix when a capture was played, before it becomes a listen — the cheapest
	 *  correction there is, because nothing is in a history yet. */
	async setCaptureTime(id: number, playedAt: number): Promise<{ saved: boolean }> {
		return this.request('PATCH', `/api/v1/patch/captures/${id}`, { played_at: playedAt });
	}

	/** An evening as a card.
	 *
	 *  The URL rather than the bytes: the caller renders it in an `<img>` and
	 *  fetches it only when saving, which is what lets the coverage header be
	 *  read. Session-only, like everything else on a sitting. */
	spoolImageUrl(
		uuid: string,
		opts: { size?: string; theme?: string; format?: string } = {}
	): string {
		const q = new URLSearchParams();
		if (opts.size) q.set('size', opts.size);
		if (opts.theme) q.set('theme', opts.theme);
		if (opts.format) q.set('format', opts.format);
		const s = q.toString();
		return `/api/v1/patch/spools/${encodeURIComponent(uuid)}/image${s ? `?${s}` : ''}`;
	}

	async getSpoolChain(): Promise<{ chain_id: number | null }> {
		return this.get('/api/v1/profile/spool-chain');
	}

	/** The chain a play accepted out of a room is filed under. Null files it
	 *  under nothing, which is the default. */
	async setSpoolChain(chainId: number | null): Promise<{ saved: boolean }> {
		return this.request('PUT', '/api/v1/profile/spool-chain', { chain_id: chainId });
	}

	/** Move listens in time by an offset.
	 *
	 *  An offset rather than an absolute time because that is the shape of the
	 *  actual failure — a clock out by an hour moves a *run* by a *constant*,
	 *  and a constant cannot scramble an ordering. For one listen, subtract.
	 *
	 *  `already_forwarded` and `grouping_stale` are the two things it cannot
	 *  reach and therefore has to report. */
	async shiftListenTimes(
		ids: number[],
		offsetSeconds: number
	): Promise<{
		moved: number;
		already_forwarded: number;
		sessions_touched: number;
		grouping_stale: boolean;
	}> {
		return this.request('POST', '/api/v1/scrobbles/bulk-time', {
			ids,
			offset_seconds: offsetSeconds
		});
	}

	/** Answering an invitation. `offered` is the backlog joining has just put
	 *  in your inbox — for a sitting that ended before you answered, that is
	 *  the whole of it. */
	async joinSpool(uuid: string, accept: boolean): Promise<{ state: string; offered: number }> {
		return this.request('POST', `/api/v1/patch/spool/${encodeURIComponent(uuid)}/join`, {
			accept
		});
	}

	/** One number for the sidebar badge — see the endpoint's note on why it is
	 *  not four requests. Never throws in practice: the server answers 0 rather
	 *  than an error, because a badge is decoration. */
	/** Who else on this instance plays an artist, and which of your own chains
	 *  you hear them on.
	 *
	 *  Instance-wide rather than narrowed by your patches — the same reversal
	 *  the Instance tab makes, on the same reasoning: the per-chain switch is
	 *  the consent, and switching it on is giving it. */
	async getPatchArtistContext(
		name: string
	): Promise<{ decks: ArtistDeck[]; chains: { name: string; plays: number }[] }> {
		return this.get(`/api/v1/patch/artist?name=${encodeURIComponent(name)}`);
	}

	/** Decks playing right now — accepted patch, shared chain. */
	async getPatchLive(): Promise<{ live: LiveDeck[] }> {
		return this.get('/api/v1/patch/live');
	}

	async getPatchInboxCount(): Promise<{ waiting: number }> {
		return this.get('/api/v1/patch/inbox-count');
	}

	async getSittings(): Promise<{ sittings: HandedSitting[] }> {
		return this.get('/api/v1/patch/sittings');
	}

	async getSitting(uuid: string): Promise<HandedSittingDetail> {
		return this.get(`/api/v1/patch/sittings/${encodeURIComponent(uuid)}`);
	}

	/**
	 * Pass a past sitting to somebody. **Two different gestures, and `kind` is
	 * required** — the server refuses to guess, because they write different
	 * things to another person's permanent record.
	 *
	 * `spool` — they were in the room and neither of you started a sitting. The
	 * plays arrive as ordinary captures and enter their history when they
	 * accept, exactly as a live Shared Spool would have.
	 *
	 * `recommendation` — they were not there. A document of what you played;
	 * nothing enters their history.
	 */
	async handOverSitting(body: {
		session: string;
		to: string;
		kind: 'spool' | 'recommendation';
		note?: string;
	}): Promise<{ kind: string; uuid: string; offered?: number; withheld?: number }> {
		return this.post('/api/v1/patch/sittings', body);
	}

	/**
	 * What you have passed on, and whether it has been read.
	 *
	 * Only documents appear — a reconstructed spool writes captures into the
	 * other person's inbox and nothing here, so there is nothing to take back.
	 */
	async getSentSittings(): Promise<{ sittings: SentSitting[] }> {
		return this.get('/api/v1/patch/sittings/sent');
	}
	async withdrawSitting(uuid: string): Promise<{ withdrawn: boolean }> {
		return this.request('DELETE', `/api/v1/patch/sittings/${encodeURIComponent(uuid)}`);
	}

	/** Not an accept — it wrote nothing. Just stops showing in the inbox. */
	async markSittingSeen(uuid: string): Promise<{ seen: boolean }> {
		return this.post(`/api/v1/patch/sittings/${encodeURIComponent(uuid)}/seen`, {});
	}

	async endSpool(uuid: string): Promise<{ ended: boolean }> {
		return this.request('POST', `/api/v1/patch/spool/${encodeURIComponent(uuid)}/end`, {});
	}

	/** `forwarded` is how many of the settled plays will be relayed onward. Not
	 *  derivable from `settled` — it turns on the user's switch *and* on each
	 *  play being inside the freshness window. */
	async settleCaptures(
		ids: number[],
		accept: boolean
	): Promise<{ settled: number; forwarded: number }> {
		return this.request('POST', '/api/v1/patch/captures', { ids, accept });
	}

	async getReel(
		params: {
			limit?: number;
			before?: number;
			/** Narrows on track, artist, album, chain or deck. **Server-side**:
			 *  the Reel is keyset-paged, so filtering the loaded page would
			 *  silently search only what has been scrolled to. */
			q?: string;
			/** One deck's username. */
			deck?: string;
		} = {}
	): Promise<ReelPage> {
		const q = new URLSearchParams();
		if (params.limit != null) q.set('limit', String(params.limit));
		if (params.before != null) q.set('before', String(params.before));
		if (params.q) q.set('q', params.q);
		if (params.deck) q.set('deck', params.deck);
		const qs = q.toString();
		return this.get(`/api/v1/patch/reel${qs ? `?${qs}` : ''}`);
	}

	// ── Patch: whether this deck answers to the fediverse at all ──
	//
	// A **different question** from the now-playing switch above, and the two
	// must not be folded together: that one decides whether strangers can read
	// what is on the deck, this one decides whether the person can be *found*
	// and patched into. Following happens while private in this design, so a
	// deck can be discoverable and unreadable at the same time.
	//
	// Session-only on the server, same as the switch above: a token gets 401.
	/** Whether decks you have patched with see all your listens. */
	async getShareAll(): Promise<{ enabled: boolean }> {
		return this.get('/api/v1/profile/share-all');
	}
	async setShareAll(enabled: boolean): Promise<{ enabled: boolean }> {
		return this.put('/api/v1/profile/share-all', { enabled });
	}
	async getFederation(): Promise<FederationSwitch> {
		return this.get('/api/v1/profile/federation');
	}

	async setFederation(enabled: boolean): Promise<FederationSwitch> {
		return this.request('PUT', '/api/v1/profile/federation', { enabled });
	}

	// ── The publish switch for now-playing ──
	//
	// Session-only on the server: a token gets 401, because a scrobble client
	// has no business deciding that its owner's listening is public.
	async getPublicNowPlaying(): Promise<PublishSwitch> {
		return this.get('/api/v1/profile/public-now-playing');
	}

	async setPublicNowPlaying(body: {
		enabled: boolean;
		fields: string[];
	}): Promise<PublishSwitch> {
		return this.request('PUT', '/api/v1/profile/public-now-playing', body);
	}

	async setTidalSettings(body: {
		client_id?: string;
		client_secret?: string;
	}): Promise<{ configured: boolean }> {
		return this.request('PUT', '/api/v1/tidal/settings', body);
	}

	// ── Import ──
	async importListenBrainz(username: string, token?: string): Promise<{ job_id: string }> {
		return this.post('/api/v1/import/listenbrainz', { username, token });
	}
	async importLastfm(username: string): Promise<{ job_id: string }> {
		return this.post('/api/v1/import/lastfm', { username });
	}
	/** `timezone` is only consulted for a Rockbox log whose header says #TZ/UNKNOWN. */
	async importFile(file: File, timezone?: string): Promise<{ job_id: string }> {
		const form = new FormData();
		if (timezone) form.append('timezone', timezone);
		form.append('file', file);
		const res = await fetch(`${BASE}/api/v1/import/file`, {
			method: 'POST', credentials: 'same-origin', body: form
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}
	/** Jobs still running for this user — lets the page pick a live import back
	 *  up after you navigate away and return. */
	async activeImportJobs(): Promise<{ jobs: ImportJob[] }> { return this.get('/api/v1/import/jobs'); }
	async importJob(jobId: string): Promise<ImportJob> { return this.get(`/api/v1/import/jobs/${jobId}`); }
	async importHistory(): Promise<{ imports: ImportRecord[] }> { return this.get('/api/v1/import/history'); }

	// ── Chains ──
	async getChains(): Promise<{ chains: SignalChain[] }> { return this.get('/api/v1/chains'); }
	async getChain(id: number): Promise<SignalChain> { return this.get(`/api/v1/chains/${id}`); }
	async createChain(data: ChainInput): Promise<{ id: number }> {
		return this.post('/api/v1/chains', data);
	}
	async updateChain(id: number, data: ChainInput): Promise<{ id: number }> {
		return this.patch(`/api/v1/chains/${id}`, data);
	}
	async deleteChain(id: number): Promise<void> { return this.delNoBody(`/api/v1/chains/${id}`); }

	// ── Devices & Equipment ──
	// ── Metadata sanitiser ──
	async sanitizePreview(): Promise<SanitizePreview> { return this.get('/api/v1/sanitize'); }
	/** Booleans mean "everything in this category"; the `*_edits` lists name
	 *  individual changes and take precedence over their boolean. */
	async sanitizeApply(opts: {
		albums?: boolean; artists?: boolean; variants?: boolean;
		album_edits?: { artist: string; album: string; mbid_release: string }[];
		credit_edits?: { artist: string }[];
		variant_edits?: { from: string; to: string }[];
		splits?: boolean;
		split_edits?: { title: string; album: string; from: string; to: string }[];
	}): Promise<SanitizeResult> {
		return this.post('/api/v1/sanitize', opts);
	}

	// ── Rediscovery ──
	async rediscovery(quietDays?: number): Promise<Rediscovery> {
		return this.get(`/api/v1/rediscovery${quietDays ? `?quiet_days=${quietDays}` : ''}`);
	}
	async neverOnGear(equipmentId: number): Promise<{ rows: RediscoveryRow[] }> {
		return this.get(`/api/v1/rediscovery/gear/${equipmentId}`);
	}

	// ── Playlist Lab ──
	async playlistLab(minPlays?: number): Promise<PlaylistLab> {
		const qs = minPlays ? `?min_plays=${minPlays}` : '';
		const raw = await this.get<{
			dims: LabDim[];
			tracks: [string, string, string | null, number, number[]][];
			loved: number[];
			pool: number;
			considered: number;
			unplaced: number;
			min_plays: number;
			granularity: string;
			note: string;
		}>(`/api/v1/playlist-lab${qs}`);
		// Sent as arrays rather than objects — on a 3,000-track pool the
		// repeated key names would be most of the payload. Unpacked once here so
		// nothing downstream has to know the wire shape.
		return {
			...raw,
			tracks: raw.tracks.map(([title, artist, album, plays, vector]) => ({
				title,
				artist,
				album,
				plays,
				vector
			}))
		};
	}

	// ── Pushing a generated list to a media server ──
	async playlistTargets(): Promise<{ targets: PlaylistTarget[] }> {
		return this.get('/api/v1/playlist-targets');
	}
	async pushPlaylist(
		source_id: number,
		name: string,
		tracks: PlaylistPushTrack[]
	): Promise<PlaylistPushResult> {
		return this.post('/api/v1/playlists', { source_id, name, tracks });
	}

	/** Pin a track to one of the candidates a push offered. Recorded as a
	 *  *manual* resolution: it never expires, and a later automatic guess will
	 *  not overwrite it. */
	async setPlaylistMatch(body: {
		source_id: number;
		artist: string;
		title: string;
		remote_id: string;
		remote_label?: string;
	}): Promise<{ status: string }> {
		return this.post('/api/v1/playlist-matches', body);
	}

	// ── Physical media: records and tapes ──
	async lookupPressing(q: {
		barcode?: string; catalog_number?: string; artist?: string; release?: string; kind?: string;
	}): Promise<{ candidates: ReleaseCandidate[]; discogs_enabled: boolean; notes: string[] }> {
		const p = new URLSearchParams();
		for (const [k, v] of Object.entries(q)) if (v) p.set(k, v);
		return this.get(`/api/v1/physical/lookup?${p}`);
	}
	async discogsRelease(id: string): Promise<{ candidate: ReleaseCandidate }> {
		return this.get(`/api/v1/physical/discogs/${encodeURIComponent(id)}`);
	}
	async listShelf(kind?: string): Promise<{ releases: PhysicalRelease[] }> {
		return this.get(`/api/v1/physical${kind ? `?kind=${kind}` : ''}`);
	}
	async getPressing(id: number): Promise<{ release: PhysicalRelease }> {
		return this.get(`/api/v1/physical/${id}`);
	}
	async addPressing(
		body: Partial<ReleaseCandidate> & {
			kind: string; tracks: PhysicalTrack[];
			purchased_at?: number | null;
			acquired_as?: string | null;
			acquired_from?: string | null;
			/** Pull the sleeve, labels and booklet from the Cover Art Archive as
			 *  part of adding it. Defaults to on server-side. */
			fetch_images?: boolean;
		}
	) {
		return this.post<{ id: number; images: number }>('/api/v1/physical', body);
	}
	/** Edit the shelf-keeping fields. Every one distinguishes absent from null:
	 *  omit to leave alone, pass `null` to clear. */
	/** The words to one track, as numbered lines — blanks included, because a
	 *  note points at a line by its index. Reads only what is stored; the
	 *  backfill is what talks to LRCLIB. */
	async lyrics(artist: string, title: string, album?: string) {
		const q = new URLSearchParams({ artist, title });
		if (album) q.set('album', album);
		return this.get<LyricDoc>(`/api/v1/lyrics?${q}`);
	}

	/** The words for whatever is playing, in one request.
	 *
	 *  What makes the theatre stateful: it is asked once when the reader opens
	 *  and again only when the deck's track changes, so the words follow the
	 *  music without the page being addressed by URL — and without a document
	 *  this size being pulled every three seconds. */
	async liveLyrics() {
		return this.get<LiveLyricDoc>('/api/v1/lyrics/live');
	}

	/** Look for this track's words now, rather than waiting for the backfill.
	 *
	 *  Their own library first, then LRCLIB. Answers with the whole document so
	 *  the reader can simply replace what it is showing; `unavailable` with a
	 *  `why` means the upstream asked us to wait, which is not the same as
	 *  having found nothing and must not be reported as it. */
	async fetchLyricsNow(body: {
		artist: string;
		title: string;
		album?: string;
		duration?: number;
	}) {
		return this.post<LyricDoc & { already_held?: boolean; unavailable?: boolean; why?: string }>(
			'/api/v1/lyrics/fetch',
			body
		);
	}

	/** Every track this instance holds words for, as `artist|title` keys.
	 *
	 *  One request for the whole set rather than a lookup per row — the shape
	 *  `/loves/keys` already has. Derive a row's key with `lyricKey`, and keep
	 *  that in step with the server's fold or "Read the words" quietly stops
	 *  appearing and nothing errors. */
	async lyricKeys() {
		return this.get<{ keys: string[] }>('/api/v1/lyrics/keys');
	}

	/** Every lyric line this user has pinned, newest edit first — across
	 *  everything, which is what makes it a way back into writing you had
	 *  forgotten rather than a second copy of the margin in front of you. */
	async pinnedLines() {
		return this.get<{ lines: PinnedLine[] }>('/api/v1/lines');
	}

	/** Correct the words by hand. A value you supplied is never overwritten by
	 *  a later lookup — and `renumbered` says when the line count changed,
	 *  which re-points every quote past the change. */
	async fixLyrics(body: { artist: string; title: string; plain?: string; synced?: string }) {
		return this.put<{
			status: string;
			language: string | null;
			lines_before: number;
			lines_after: number;
			renumbered: boolean;
		}>('/api/v1/lyrics', body);
	}

	async updatePressing(
		id: number,
		body: {
			purchased_at?: number | null;
			chain_id?: number | null;
			equipment_id?: number | null;
			notes?: string | null;
			/** Move it to another shelf. Changing this rewrites the tracklist's
			 *  sides — a record has a break, a disc has none — so the response
			 *  reports `sides_rewritten` and the caller should refetch. */
			kind?: ShelfKind;
			/** The MusicBrainz release this pressing is, typed in by hand. Also
			 *  the cheapest running-time lookup — one request, no search. */
			mbid_release?: string | null;
			/** How it came to be yours, and from where. Both clearable — a wrong
			 *  answer is worse than a blank. */
			acquired_as?: string | null;
			acquired_from?: string | null;
		}
	): Promise<{ status: string; sides_rewritten?: boolean }> {
		return this.patch(`/api/v1/physical/${id}`, body);
	}
	/** What another pressing says this record's tracks run. Preview only. */
	async previewDurations(id: number): Promise<DurationPreview> {
		return this.get(`/api/v1/physical/${id}/durations`);
	}

	/** Write back exactly what the preview showed. */
	async applyDurations(
		id: number,
		durations: DurationProposal[]
	): Promise<{ filled: number; listens_updated: number }> {
		return this.post(`/api/v1/physical/${id}/durations`, { durations });
	}

	/**
	 * Replace the tracklist — and, with it, the running times.
	 *
	 * The counterpart of `applyDurations` for the case no provider can answer:
	 * a pressing MusicBrainz does not have stays unknown for ever otherwise,
	 * which means the deck shows no length and its listens contribute nothing
	 * to gear hours. The server corrects the listens and the recorded wear
	 * alongside, so all three cannot disagree.
	 */
	async setPressingTracks(
		id: number,
		tracks: PhysicalTrack[]
	): Promise<{
		status: string;
		changed: number;
		listens_updated: number;
		plays_recomputed: number;
	}> {
		return this.request('PUT', `/api/v1/physical/${id}/tracks`, { tracks });
	}
	async deletePressing(id: number) {
		return this.delNoBody(`/api/v1/physical/${id}`);
	}
	async playSide(
		id: number,
		body: {
			side: string; started_at?: number;
			equipment_id?: number | null; chain_id?: number | null;
			/** Per-track outcomes from the deck. The browser ran the clock, so
			 *  it is the only thing that saw the pauses and the skips. */
			tracks?: { position: number; started_at: number; skipped: boolean }[];
			/** The first track that actually played. Without it every track
			 *  before the needle is recorded as having played — and they must
			 *  not become skips either, since a track you never reached is not
			 *  one you skipped. */
			from_position?: number;
		/** Last track played — with `from_position`, a range; equal, one track. */
		to_position?: number;
			/** Where a tape ended up, in seconds into the side. Cassettes only;
			 *  the server refuses it for anything else, because a record has no
			 *  such state. Absent means the side ran through and wound on. */
			parked_secs?: number;
		}
	): Promise<PlayResult> {
		return this.post(`/api/v1/physical/${id}/play`, body);
	}
	async equipmentWear(id: number): Promise<{ hours: number; sides_played: number }> {
		return this.get(`/api/v1/physical/equipment/${id}/wear`);
	}

	/** How often each side has been played, and for how long. */
	async sidePlays(id: number): Promise<{ sides: SidePlays[] }> {
		return this.get(`/api/v1/physical/${id}/sides`);
	}

	// ── The rest of the sleeve ──
	async pressingImages(id: number): Promise<{ images: PhysicalImage[] }> {
		return this.get(`/api/v1/physical/${id}/images`);
	}
	/** Ask the Cover Art Archive (and Discogs, if configured) again. Idempotent
	 *  by the provider's own image id, so this adds what is new and leaves the
	 *  rest alone. */
	async fetchPressingImages(
		id: number
	): Promise<{ stored: number; total: number; notes: string[] }> {
		return this.post(`/api/v1/physical/${id}/images/fetch`, {});
	}
	/** Upload scans of your own copy. `kind` and `caption` apply to all the
	 *  files in one call, which is what makes uploading a booklet in one go
	 *  work — they land in page order. */
	async uploadPressingImages(
		id: number,
		files: File[],
		kind: string,
		caption?: string
	): Promise<{ ids: number[] }> {
		const form = new FormData();
		form.append('kind', kind);
		if (caption) form.append('caption', caption);
		for (const f of files) form.append('file', f);
		const res = await fetch(`${BASE}/api/v1/physical/${id}/images`, {
			method: 'POST', credentials: 'same-origin', body: form
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}
	async updatePressingImage(
		imageId: number,
		body: { kind?: string; caption?: string | null; position?: number }
	) {
		return this.patch(`/api/v1/physical/images/${imageId}`, body);
	}
	async deletePressingImage(imageId: number) {
		return this.del(`/api/v1/physical/images/${imageId}`);
	}

	// ── What's on the deck ──
	//
	// The shelf is the one source where the *browser* is the player: the clock
	// runs in front of the turntable, so nothing server-side can derive a
	// playhead. It pushes instead.
	async shelfNowPlaying(
		id: number,
		body: { side: string; position: number; elapsed: number; paused: boolean }
	) {
		return this.post(`/api/v1/physical/${id}/now-playing`, body);
	}
	async clearShelfNowPlaying() {
		return this.del('/api/v1/physical/now-playing');
	}

	// ── The side on the deck, held server-side ──
	//
	// The countdown still runs in the page — nothing else can watch a
	// turntable — but *what is on* and *when it started* live on the server.
	// That is what lets a second device see the record, and what makes a side
	// survive the browser crashing.
	async getSpin(): Promise<{ spin: Spin | null }> {
		return this.get('/api/v1/physical/spin');
	}
	async putSpin(body: SpinUpdate): Promise<{ status: string }> {
		return this.put('/api/v1/physical/spin', body);
	}
	async clearSpin(): Promise<{ status: string }> {
		return this.del('/api/v1/physical/spin');
	}
	/** Set or clear an artist portrait by hand. Empty clears it. */
	async setArtistImage(artistRowId: number, url: string): Promise<{ image_url: string | null }> {
		return this.request('PUT', `/api/v1/artists/img/${artistRowId}`, { url });
	}
	/** Set or clear an artist's **wide** image by hand. Empty clears it.
	 *
	 *  A different photograph from the portrait, not a crop of it: the Reports
	 *  chapter tile is 16/11, where a square either pillarboxes or loses the
	 *  head. Nothing supplies these automatically, so it is always a choice. */
	async setArtistLandscape(
		artistRowId: number,
		url: string
	): Promise<{ landscape_url: string | null }> {
		return this.request('PUT', `/api/v1/artists/landscape/${artistRowId}`, { url });
	}
	/** Set or clear an album cover by hand — takes the *entity* id, the same one
	 *  the album page is addressed by. Empty restores the cover derived from the
	 *  listens. */
	async setAlbumCover(entityId: number, url: string): Promise<{ artwork_url: string | null }> {
		return this.request('PUT', `/api/v1/albums/${entityId}/cover`, { url });
	}

	/** Upload a picture instead of pasting a URL.
	 *
	 *  The three overrides above cover a picture that is already somewhere on
	 *  the web. A photograph of a record you own, or a band shot that exists
	 *  only on your disk, had no route in at all — and "host it somewhere
	 *  first" is not an answer for a self-hosted journal.
	 *
	 *  These replace the override exactly as a pasted URL does, and the file
	 *  the new one supersedes is deleted server-side. */
	async uploadArtistImage(artistRowId: number, file: File): Promise<{ image_url: string }> {
		return this.uploadArt(`/api/v1/artists/img/${artistRowId}/upload`, file);
	}
	async uploadArtistLandscape(
		artistRowId: number,
		file: File
	): Promise<{ landscape_url: string }> {
		return this.uploadArt(`/api/v1/artists/landscape/${artistRowId}/upload`, file);
	}
	async uploadAlbumCover(entityId: number, file: File): Promise<{ artwork_url: string }> {
		return this.uploadArt(`/api/v1/albums/${entityId}/cover/upload`, file);
	}
	private async uploadArt(path: string, file: File) {
		const form = new FormData();
		form.append('file', file);
		const res = await fetch(`${BASE}${path}`, {
			method: 'POST', credentials: 'same-origin', body: form
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}

	async getDiscogsSetting(): Promise<{
		configured: boolean;
		from_env: boolean;
		stored: boolean;
		/** Which credential shape is in effect. */
		kind: 'token' | 'key_secret' | 'none';
	}> {
		return this.get('/api/v1/physical/settings/discogs');
	}
	/** Only the supplied fields are written, so saving one shape doesn't wipe the other. */
	async setDiscogsSetting(patch: {
		token?: string;
		key?: string;
		secret?: string;
	}): Promise<{ configured: boolean }> {
		return this.request('PUT', '/api/v1/physical/settings/discogs', patch);
	}
	/** Does the credential actually work? Discogs answers a bad one with a normal 200. */
	async testDiscogsSetting(): Promise<{ ok: boolean; message: string }> {
		return this.post('/api/v1/physical/settings/discogs/test');
	}

	// ── AudioMuse-AI (amai), the sonic sidecar ──────────────────────────────
	//
	// Admin and session-only. amai analyses the audio of a media-server library
	// and holds a per-recording vector, which is the one thing this application
	// cannot derive from its own data — see `src/amai.rs`.

	async getAmaiSetting(): Promise<{
		configured: boolean;
		/** Shown; it is an address, not a credential. The token never comes back. */
		url: string | null;
		has_token: boolean;
		/** Which of amai's media servers our provider ids belong to. */
		server: string | null;
		stored: boolean;
		from_env: boolean;
	}> {
		return this.get('/api/v1/amai/settings');
	}

	/** Only the supplied fields are written, so saving one doesn't wipe another. */
	async setAmaiSetting(patch: {
		url?: string;
		token?: string;
		server?: string;
	}): Promise<{ configured: boolean }> {
		return this.request('PUT', '/api/v1/amai/settings', patch);
	}

	/**
	 * Reachable, and does the token work? Two answers, not one: amai's auth
	 * barrier exempts `/api/health`, so "running with a wrong token" and "not
	 * running" are distinguishable and need different things doing about them.
	 */
	async testAmai(): Promise<{
		ok: boolean;
		reachable: boolean;
		authenticated: boolean;
		message: string;
		servers?: { server_id: string; name: string; server_type: string; is_default: boolean }[];
	}> {
		return this.post('/api/v1/amai/test');
	}

	/**
	 * How much of this history amai can answer for.
	 *
	 * `joinable_*` is the structural ceiling — a listen that never came off a
	 * media server has no provider id to join on, whatever amai has analysed.
	 * The `analysed_*` figures are measured on the most-played sample, and
	 * `sample_share_of_listening` is what stops them being read as facts about
	 * the whole history.
	 */
	async getAmaiCoverage(sample?: number): Promise<{
		total_plays: number;
		distinct_tracks: number;
		joinable_tracks: number;
		joinable_plays: number;
		joinable_share_of_listening: number | null;
		asked: number;
		analysed: number;
		analysed_share_of_asked: number | null;
		analysed_share_of_sampled_listening: number | null;
		sample_share_of_listening: number | null;
		sample_requested: number;
		deadline_hit: boolean;
		error: string | null;
		server_asked: string | null;
		examples_not_analysed: { artist: string; title: string; plays: number }[];
	}> {
		return this.get(`/api/v1/amai/coverage${sample ? `?sample=${sample}` : ''}`);
	}

	/** Genres bordering the caller's listening. */
	async getGenreNeighbours(): Promise<GenreNeighbours> {
		return this.get('/api/v1/genre-map/neighbours');
	}

	/** Where the caller's listening sat, year by year. */
	async getGenreTrajectory(): Promise<GenreTrajectory> {
		return this.get('/api/v1/genre-map/trajectory');
	}

	/** The whole genre map, and where the caller's listening sits on it. */
	async getGenreMap(): Promise<GenreMap> {
		return this.get('/api/v1/genre-map');
	}

	// ── Operations (admin) ──
	//
	// Instance-wide health, and the log. Admin- and session-only server-side:
	// a log carries usernames, source labels and request paths, so an API token
	// cannot reach any of this.
	async getOps(): Promise<OpsStatus> {
		return this.get('/admin/ops');
	}
	async getLogFiles(): Promise<{ files: LogFile[] }> {
		return this.get('/admin/logs/files');
	}
	async getLogTail(opts: {
		lines?: number;
		level?: string;
		q?: string;
		file?: string;
	} = {}): Promise<LogTail> {
		const p = new URLSearchParams();
		if (opts.lines) p.set('lines', String(opts.lines));
		if (opts.level) p.set('level', opts.level);
		if (opts.q) p.set('q', opts.q);
		if (opts.file) p.set('file', opts.file);
		const qs = p.toString();
		return this.get(`/admin/logs${qs ? `?${qs}` : ''}`);
	}
	/// The download is a plain link rather than a fetch — the file is served as
	/// an attachment and the browser is better at saving one than we are.
	logDownloadUrl(file?: string): string {
		return `/admin/logs/download${file ? `?file=${encodeURIComponent(file)}` : ''}`;
	}
	async restartServer(): Promise<{ status: string; note: string }> {
		return this.post('/admin/restart');
	}

	// ── Every Noise genre map (admin) ──
	async getEveryNoise(): Promise<EveryNoiseStatus> {
		return this.get('/admin/everynoise');
	}
	async setEveryNoiseContact(contact: string): Promise<{ status: string }> {
		return this.request('PUT', '/admin/everynoise/contact', { contact });
	}
	async startEveryNoiseScrape(tier: 'map' | 'pages'): Promise<{ status: string }> {
		return this.post('/admin/everynoise/scrape', { tier });
	}
	async cancelEveryNoiseScrape(): Promise<{ status: string }> {
		return this.post('/admin/everynoise/cancel');
	}
	async clearEveryNoise(): Promise<{ status: string }> {
		return this.del('/admin/everynoise');
	}

	// ── Manual listen entry ──
	/**
	 * Submit listens through the ListenBrainz-compatible ingest endpoint.
	 *
	 * The same route external scrobble clients use — it already accepts a
	 * browser session, so a hand-entered listen goes through exactly the
	 * validation, deduplication and chain resolution that a real one does
	 * rather than a parallel path that could drift from it.
	 */
	async submitListens(payload: unknown[], listen_type: 'single' | 'import' = 'single') {
		return this.post('/1/submit-listens', { listen_type, payload });
	}

	// ── Search ──
	async search(q: string, limit = 10): Promise<SearchResults> {
		return this.get(`/api/v1/search?q=${encodeURIComponent(q)}&limit=${limit}`);
	}

	// ── Sessions ──
	async getSessions(limit = 50, offset = 0): Promise<{ sessions: Session[]; count: number }> {
		return this.get(`/api/v1/sessions?limit=${limit}&offset=${offset}`);
	}
	async getSession(id: number): Promise<{ session: Session; scrobbles: Scrobble[] }> {
		return this.get(`/api/v1/sessions/${id}`);
	}
	/** Two or more ids; the earliest survives and absorbs the rest. */
	async mergeSessions(ids: number[]): Promise<{ session: Session }> {
		return this.post('/api/v1/sessions/merge', { ids });
	}
	/** `at_scrobble_id` becomes the first listen of the new session. */
	async splitSession(id: number, at_scrobble_id: number): Promise<{ session: Session; new_session: Session }> {
		return this.post(`/api/v1/sessions/${id}/split`, { at_scrobble_id });
	}
	/** Regroup every listen from scratch. Discards manual merges and splits. */
	/** Start a regroup. Returns a job id to poll — the work runs in the
	 *  background because on a real history it ran past a reverse proxy's
	 *  origin timeout, and the browser got a 524 while the rebuild carried on
	 *  unseen. A 409 means one is already running and carries its id. */
	async rebuildSessions(gap_seconds?: number): Promise<{ job_id: string; state: string }> {
		return this.post('/api/v1/sessions/rebuild', gap_seconds ? { gap_seconds } : {});
	}
	async rebuildStatus(jobId: string): Promise<{
		job_id: string;
		state: 'running' | 'done' | 'error';
		sessions: number;
		listens: number;
		status: string;
		started_at: number;
	}> {
		return this.get(`/api/v1/sessions/rebuild/${jobId}`);
	}
	async updateSession(
		id: number,
		data: {
			chain_id?: number | null;
			/** Which variant of that chain — the "I had the other IEMs in"
			 *  edit. Rewritten whenever `chain_id` is, since a variant belongs
			 *  to exactly one chain. */
			chain_variant_id?: number | null;
			title?: string | null;
			notes?: string | null;
		}
	): Promise<{ session: Session }> {
		return this.patch(`/api/v1/sessions/${id}`, data);
	}

	// ── Charts ──
	async getChart(kind: ChartKind, range: ChartRange, limit = 100, metric: 'plays' | 'hours' = 'plays'): Promise<{ kind: ChartKind; range: string; rows: ChartRow[] }> {
		return this.get(`/api/v1/charts?kind=${kind}&range=${range}&limit=${limit}&metric=${metric}`);
	}

	/** Fetch loved tracks from every connected service. */
	async pullLoves(): Promise<{ loved: number; services: { service: string; found: number; loved: number; skipped: number; error: string | null }[] }> {
		return this.post('/api/v1/loves/pull', {});
	}

	// ── Library star ratings ──
	//
	// The raw number and `scale_max` travel together and neither means anything
	// alone: Plex counts to ten, Subsonic to five. `shown` is the same rating on
	// the display scale, and is presentation only — the server compares the
	// underlying fraction, so a rating that *reads* as four stars on a coarse
	// scale has not thereby cleared a four-star bar.
	async getRatings(): Promise<RatingsView> {
		return this.get('/api/v1/ratings');
	}
	/** Read what every configured library server holds. Stores; loves nothing. */
	async pullRatings(): Promise<{ found: number; sources: RatingSourcePull[] }> {
		return this.post('/api/v1/ratings/pull', {});
	}
	/** Love what clears the bar, and withdraw what no longer does.
	 *  `dry_run` is required by the server — the two things this can do are
	 *  "tell me what would happen" and "write to my Loved page". */
	async applyRatings(req: {
		dry_run: boolean;
		threshold?: number;
		scale?: string;
		include_starred?: boolean;
		save?: boolean;
	}): Promise<RatingApplyResult> {
		return this.post('/api/v1/ratings/apply', req);
	}
	/** Forget the stored ratings. Does not un-love anything. */
	async clearRatings(): Promise<{ forgotten: number; note: string }> {
		return this.del('/api/v1/ratings');
	}

	// ── Album & artist pages ──
	async getAlbum(id: number): Promise<AlbumView> { return this.get(`/api/v1/albums/${id}`); }
	async getArtist(id: number): Promise<ArtistView> { return this.get(`/api/v1/artists/${id}`); }
	/** The record as MusicBrainz has it, including tracks never played. 404s when
	 *  MusicBrainz has no matching release, which is normal for a great deal of a
	 *  real library and is not an error worth shouting about. */
	async getAlbumTracklist(id: number): Promise<AlbumTracklist> { return this.get(`/api/v1/albums/${id}/tracklist`); }
	/** Name → entity id, creating it on first use. See `$lib/entities`. */
	async resolveEntity(kind: 'album' | 'artist' | 'track', name: string, artist?: string): Promise<{ id: number; kind: string }> {
		const q = new URLSearchParams({ kind, name });
		if (artist) q.set('artist', artist);
		return this.get(`/api/v1/resolve?${q}`);
	}

	async getDevices(): Promise<{ devices: Device[] }> { return this.get('/api/v1/devices'); }
	async setDeviceChain(id: number, chain_id: number | null): Promise<void> {
		await this.patch(`/api/v1/devices/${id}`, { chain_id });
	}
	async getEquipment(): Promise<{ equipment: Equipment[] }> { return this.get('/api/v1/equipment'); }
	async createEquipment(data: EquipmentInput): Promise<{ id: number }> {
		return this.post('/api/v1/equipment', data);
	}
	async updateEquipment(id: number, data: EquipmentInput): Promise<{ id: number }> {
		return this.patch(`/api/v1/equipment/${id}`, data);
	}
	async deleteEquipment(id: number): Promise<void> { return this.delNoBody(`/api/v1/equipment/${id}`); }

	// ── Output-device → chain bindings ──
	async getBindings(): Promise<{ bindings: Binding[] }> { return this.get('/api/v1/bindings'); }
	async setBinding(identifier: string, chain_id: number | null): Promise<void> {
		await this.request('PUT', '/api/v1/bindings', { identifier, chain_id });
	}
	async deleteBinding(id: number): Promise<void> { return this.delNoBody(`/api/v1/bindings/${id}`); }

	// ── Bulk chain reassignment (History) ──
	async bulkSetChain(
		ids: number[],
		signal_chain_id: number | null,
		chain_variant_id: number | null = null
	): Promise<{ updated: number }> {
		return this.post('/api/v1/scrobbles/bulk-chain', { ids, signal_chain_id, chain_variant_id });
	}

	/** Mark listens as heard in company, or pass null to unmark them.
	 *
	 *  `company` is always sent — the server refuses an absent field rather
	 *  than reading it as "clear", so a typo cannot silently unmark a history. */
	async bulkSetCompany(ids: number[], company: string | null): Promise<{ updated: number }> {
		return this.post('/api/v1/scrobbles/bulk-company', { ids, company });
	}

	/** Make a setup the chain, and the chain a setup. A relabelling: what was
	 *  fitted for any listen, and every piece of gear's hours, are unchanged. */
	async promoteVariant(id: number, name?: string): Promise<{ old_base_variant_id: number }> {
		return this.post(`/api/v1/chains/variants/${id}/promote`, { name });
	}

	// ── Liner notes & loves ──
	async getNotes(params?: NoteQuery): Promise<{ notes: Note[]; total: number }> {
		const p = new URLSearchParams();
		if (params?.q?.trim()) p.set('q', params.q.trim());
		if (params?.kind) p.set('kind', params.kind);
		if (params?.sort && params.sort !== 'updated') p.set('sort', params.sort);
		const qs = p.toString();
		return this.get(`/api/v1/notes${qs ? `?${qs}` : ''}`);
	}

	/** Every note as Markdown — a zip of one file per note, or one document. */
	async notesExportBlob(format: 'zip' | 'md'): Promise<Blob> {
		const res = await fetch(`${BASE}/api/v1/notes/export?format=${format}`, {
			credentials: 'same-origin'
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.blob();
	}
	async getNote(id: number): Promise<Note> { return this.get(`/api/v1/notes/${id}`); }
	async saveNote(body: NoteTarget & { body_md: string; visibility?: Visibility }): Promise<Note> {
		return this.post('/api/v1/notes', body);
	}
	async deleteNote(id: number): Promise<void> { await this.del(`/api/v1/notes/${id}`); }
	async noteRevisions(id: number): Promise<{ revisions: NoteRevision[] }> {
		return this.get(`/api/v1/notes/${id}/revisions`);
	}
	/** Render without saving, so the editor can show what a shortcode resolves to. */
	async previewNote(body: NoteTarget & { body_md: string }): Promise<{ body_html: string }> {
		return this.post('/api/v1/notes/preview', body);
	}
	/** The note for a target, if any — plus the resolved target itself. */
	async noteFor(target: NoteTarget): Promise<{ note: Note | null; target: { entity?: Entity | null; listen?: NoteListen | null; period?: NotePeriod | null }; loved: boolean }> {
		const q = new URLSearchParams();
		for (const [k, v] of Object.entries(target)) {
			if (v !== undefined && v !== null) q.set(k, String(v));
		}
		return this.get(`/api/v1/notes/for?${q}`);
	}
	async getLoves(): Promise<{ loves: LovedRow[] }> {
		return this.get('/api/v1/loves');
	}
	/** Identity keys of everything loved, so rows can render a heart without
	 *  one request each. See `loveKey()` in `$lib/loves`. */
	async getLoveKeys(): Promise<{ keys: string[] }> {
		return this.get('/api/v1/loves/keys');
	}
	async setLove(target: NoteTarget, loved: boolean): Promise<{ entity: Entity; loved: boolean }> {
		return this.post('/api/v1/loves', { ...target, loved });
	}
	async getEntity(id: number): Promise<{ entity: Entity; loved: boolean; note: Note | null }> {
		return this.get(`/api/v1/entities/${id}`);
	}
	/** Notes and loves around one listen. Read-only — nothing is created. */
	async listenAnnotations(id: number): Promise<ListenAnnotations> {
		return this.get(`/api/v1/scrobbles/${id}/annotations`);
	}

	// ── Users (admin) ──
	async getUsers(): Promise<{ users: User[] }> {
		return this.get('/admin/users');
	}
	async createUser(username: string, display_name?: string, password?: string): Promise<{ user_id: number; username: string; token: string; message: string }> {
		return this.post('/admin/users', { username, display_name, password });
	}
	async updateUser(id: number, changes: { role?: string; password?: string; disabled?: boolean }): Promise<{ status: string }> {
		return this.patch(`/admin/users/${id}`, changes);
	}
	async deleteUser(id: number): Promise<void> {
		return this.delNoBody(`/admin/users/${id}`);
	}

	/** Put a shelf back from an export.
	 *
	 *  Not a database restore: it brings back the *objects*. The listens those
	 *  sides generated are not in a shelf export, so chain hours do not return
	 *  with them — the response says so. Matched before inserting, so importing
	 *  the same archive twice changes nothing the second time. */
	async importShelf(file: File): Promise<{
		releases: number; already_on_the_shelf: number; skipped: number;
		gear: number; images: number; missing_scans: number; plays: number; note: string;
	}> {
		const form = new FormData();
		form.append('file', file);
		const res = await fetch(`${BASE}/api/v1/physical/import`, {
			method: 'POST', credentials: 'same-origin', body: form
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}

	// ── Chain variants ──
	async getVariants(): Promise<{ variants: ChainVariant[] }> {
		return this.get('/api/v1/chains/variants');
	}
	async createVariant(body: {
		chain_id: number; name: string; overrides: VariantOverride[];
	}): Promise<{ id: number }> {
		return this.post('/api/v1/chains/variants', body);
	}
	async updateVariant(
		id: number,
		body: { name?: string; overrides?: VariantOverride[] }
	): Promise<{ status: string }> {
		return this.patch(`/api/v1/chains/variants/${id}`, body);
	}
	async deleteVariant(id: number): Promise<{ status: string }> {
		return this.del(`/api/v1/chains/variants/${id}`);
	}

	/** Chains that look like the same rig. Reports, never acts. */
	async getChainDuplicates(): Promise<{ duplicates: FoldSuggestion[] }> {
		return this.get('/api/v1/chains/duplicates');
	}
	/** Fold `id` into `into_chain_id` as a variant, moving its listening with
	 *  it and deleting it. Not reversible.
	 *
	 *  `variant_id` lands on a variant that already exists; `variant_name`
	 *  derives a new one from the difference between the two chains. One or the
	 *  other — never both, and never neither. */
	async foldChain(
		id: number,
		body: { into_chain_id: number; variant_id?: number; variant_name?: string }
	): Promise<FoldOutcome> {
		return this.post(`/api/v1/chains/${id}/fold`, body);
	}

	// ── Your own account ──
	//
	// No id in any of these — they are always the caller's. Session-only on the
	// server, so a token cannot change the credential it authenticates with.
	async getProfile(): Promise<UserProfile> {
		return this.get('/api/v1/profile');
	}
	/** Omit a field to leave it alone; pass `null` to clear it. */
	async updateProfile(body: {
		display_name?: string | null;
		bio?: string | null;
		avatar_url?: string | null;
	}): Promise<{ status: string }> {
		return this.patch('/api/v1/profile', body);
	}
	/** The current password is required — see the endpoint's own note. */
	async changePassword(current_password: string, new_password: string): Promise<{ status: string }> {
		return this.post('/api/v1/profile/password', { current_password, new_password });
	}
	async uploadAvatar(file: File): Promise<{ avatar: string }> {
		const form = new FormData();
		form.append('file', file);
		const res = await fetch(`${BASE}/api/v1/profile/avatar`, {
			method: 'POST', credentials: 'same-origin', body: form
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}
	async deleteAvatar(): Promise<{ status: string }> {
		return this.del('/api/v1/profile/avatar');
	}

	// ── Tokens ──
	async getTokens(): Promise<{ tokens: TokenInfo[] }> { return this.get('/admin/tokens'); }
	async createToken(name: string, opts?: { scopes?: string; default_chain_id?: number | null; imports_are_live?: boolean }): Promise<{ token: string; name: string; user_id: number; message: string }> {
		return this.post('/admin/tokens', {
			name,
			scopes: opts?.scopes ?? 'submit',
			default_chain_id: opts?.default_chain_id ?? null,
			imports_are_live: opts?.imports_are_live ?? false
		});
	}
	// ── Device-code pairing ──
	/** What the user is about to approve. */
	async describePairing(userCode: string): Promise<{
		client_name: string;
		scopes: string;
		can_read: boolean;
		can_write: boolean;
		approved: boolean;
		/** The account whose session started it; null for a device with no account. */
		started_by: string | null;
		/** Another account started it, so approving is refused. */
		started_by_other: boolean;
		started_secs_ago: number;
	}> {
		return this.get(`/api/v1/pair/${encodeURIComponent(userCode)}`);
	}
	/** Mint the token. It goes to the polling client, never into this response. */
	async approvePairing(userCode: string, defaultChainId: number | null): Promise<{ status: string }> {
		return this.post(`/api/v1/pair/${encodeURIComponent(userCode)}/approve`, {
			default_chain_id: defaultChainId
		});
	}

	/** Rename a token or repoint its chain. The token itself is not re-issued. */
	async updateToken(id: number, data: { name?: string; default_chain_id?: number | null; imports_are_live?: boolean }): Promise<void> {
		await this.patch(`/admin/tokens/${id}`, data);
	}
	async revokeToken(id: number): Promise<void> { await this.del(`/admin/tokens/${id}`); }
	/** Re-issue a token you've lost. Same row, new secret — clients must be
	 *  re-pasted, which is why it's separate from `updateToken`. */
	async rotateToken(id: number): Promise<{ token: string; message: string }> {
		return this.post(`/admin/tokens/${id}/rotate`, {});
	}

	// ── MCP: connections to AI assistants ──
	//
	// Deliberately separate from tokens. A token is something you paste into a
	// player you run; a connection is a standing grant to somebody else's
	// service, it carries different grants, and it can expire. Sharing a screen
	// would mean reading past six scrobble clients to revoke a chatbot.
	async getMcpConnections(): Promise<{
		connections: McpConnection[];
		endpoint: string;
		scopes: { scope: string; description: string }[];
	}> {
		return this.get('/api/v1/mcp/connections');
	}
	/** The credential is in this response and nowhere else, ever again. */
	async createMcpConnection(name: string, scopes: string): Promise<{
		id: number; name: string; scopes: string; token: string;
	}> {
		return this.post('/api/v1/mcp/connections', { name, scopes });
	}
	async revokeMcpConnection(id: number): Promise<void> {
		await this.del(`/api/v1/mcp/connections/${id}`);
	}
	async getMcpAudit(): Promise<{ calls: McpAuditCall[] }> {
		return this.get('/api/v1/mcp/audit');
	}

	// ── OAuth consent ──
	async getAuthorizeInfo(params: URLSearchParams): Promise<AuthorizeInfo> {
		return this.get(`/api/v1/oauth/authorize-info?${params.toString()}`);
	}
	async approveAuthorization(body: Record<string, string | null>): Promise<{ redirect_to: string }> {
		return this.post('/api/v1/oauth/approve', body);
	}

	// ── Saved playlists ──
	async getSavedPlaylists(): Promise<{ playlists: SavedPlaylist[] }> {
		return this.get('/api/v1/saved-playlists');
	}
	async getSavedPlaylist(id: number): Promise<{ playlist: SavedPlaylist; tracks: PlaylistTrackView[] }> {
		return this.get(`/api/v1/saved-playlists/${id}`);
	}
	async deleteSavedPlaylist(id: number): Promise<void> {
		await this.del(`/api/v1/saved-playlists/${id}`);
	}
	/** Send a stored playlist onward. A partial match is the normal outcome. */
	// ── The Crate ──
	async getCrate(): Promise<{ crate: CrateEntry[]; open: number; landed: number }> {
		return this.get('/api/v1/crate');
	}
	/**
	 * File something in your own crate.
	 *
	 * `from` is the username of a deck you are **patched into** — the Reel's
	 * "into my crate" button. The server resolves it and refuses a deck you
	 * cannot read, so the provenance the Crate then shows is earned rather than
	 * asserted; leave it out for something you found yourself.
	 *
	 * Throws on a record already in your history, and the message carries the
	 * play count — which is the useful half of that refusal.
	 */
	async addToCrate(entry: {
		kind: 'artist' | 'album' | 'track';
		artist: string;
		name?: string | null;
		mbid?: string | null;
		because: string;
		reason?: string | null;
		from?: string;
	}): Promise<{ id: number; created: boolean }> {
		return this.post('/api/v1/crate', entry);
	}
	async setCrateStatus(id: number, status: 'open' | 'acquired' | 'dismissed'): Promise<void> {
		await this.patch(`/api/v1/crate/${id}`, { status });
	}
	async deleteCrateEntry(id: number): Promise<void> {
		await this.del(`/api/v1/crate/${id}`);
	}

	async pushSavedPlaylist(id: number, sourceId: number): Promise<{
		matched: number; requested: number; protocol: string;
		missing: { title: string; artist: string }[];
	}> {
		return this.post(`/api/v1/saved-playlists/${id}/push`, { source_id: sourceId });
	}
}

export interface McpConnection {
	id: number;
	name: string;
	scopes: string;
	client_id: string | null;
	created_at: number;
	last_used_at: number | null;
	/** Null for a connection created by hand — those do not expire. */
	expires_at: number | null;
	calls: number;
}

export interface McpAuditCall {
	id: number;
	at: number;
	method: string;
	tool: string | null;
	args: string | null;
	ok: boolean;
	/** How much data left the building. Null where the answer was not a list. */
	rows_returned: number | null;
	ms: number | null;
	connection_name: string | null;
}

export interface AuthorizeInfo {
	client_name: string;
	client_id: string;
	/** Whether the client identified itself with a fetchable URL, or merely
	 *  registered a name it chose for itself. */
	verified: boolean;
	redirect_uri: string;
	/** Every grant this server defines, with the ones the client asked for
	 *  pre-selected. All six are offered so the user can widen the request —
	 *  a client can only ever ask for the minimal scope, so showing just its
	 *  request made the rest unreachable through OAuth. */
	scopes: { scope: string; description: string; requested: boolean }[];
	requested: string;
}

export interface SavedPlaylist {
	id: number;
	name: string;
	description: string | null;
	/** `user`, or the name of the connection that wrote it. */
	origin: string;
	created_at: number;
	updated_at: number;
	pushed_at: number | null;
	pushed_to: string | null;
	connection_name: string | null;
	tracks: number;
}

/**
 * The server's own sentence, out of the client's `API <status>: <body>`.
 *
 * Several refusals in this application are written to be read by a person —
 * "you already have 43 plays of this", "you are not patched into that deck" —
 * and putting the envelope in front of them is showing them our plumbing.
 * One copy, because two would drift the moment the envelope's shape changed.
 */
export function humanError(e: unknown, fallback?: string): string {
	const raw = e instanceof Error ? e.message : String(e);
	const brace = raw.indexOf('{');
	if (brace >= 0) {
		try {
			const parsed = JSON.parse(raw.slice(brace));
			if (typeof parsed?.error === 'string') return parsed.error;
		} catch {
			/* fall through to the raw text */
		}
	}
	return fallback && !raw ? fallback : raw;
}

export interface CrateEntry {
	id: number;
	kind: 'artist' | 'album' | 'track';
	artist: string;
	/** Album or track title. Null for an artist. */
	name: string | null;
	mbid: string | null;
	reason: string | null;
	/** What in your own listening prompted the suggestion. Required at the door
	 *  — a recommendation with no grounding in your history is not one. */
	because: string | null;
	/** `user`, or the name of the connection that suggested it. */
	origin: string;
	status: 'open' | 'acquired' | 'dismissed';
	created_at: number;
	acted_at: number | null;
	/** When this first turned up in your listening. Derived from the history,
	 *  not stored — it is the only way a suggestion can be scored. */
	first_played_at: number | null;
	plays: number;
}

/// A playlist track with what the history already knows about it — the artwork
/// and play count that make this page look like the rest of the application
/// rather than a text file beside Rediscovery's sleeves.
export interface PlaylistTrackView extends SavedTrack {
	/** Plays in your history. 0 for something you have never played, which is a
	 *  normal outcome — it keeps its lettered tile. */
	plays: number;
	artwork_url: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
}

export interface SavedTrack {
	title: string;
	artist: string;
	album: string | null;
	/** Why this track is here, in the words of whatever chose it. */
	note: string | null;
}

export const api = new TapedeckAPI();
