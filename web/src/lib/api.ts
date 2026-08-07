/** Tapedeck API client — cookie-based session auth for browser, Token header for external clients. */

const BASE = '';

export interface Scrobble {
	id: number;
	title: string;
	artist: string;
	album: string | null;
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
	listening_context: string | null;
	submission_client: string | null;
	/** Which code path produced it: Plex, jellyfin, navidrome, physical,
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
	top_artist: { name: string; plays: number; hours: number } | null;
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
	total_hours: number;
}

export interface ChainComponent {
	type: string;
	name: string;
	detail: string | null;
	/** The `Equipment` row this step is, when picked from your gear. `name` is
	 *  kept as a snapshot so the chain still reads if the gear is deleted. */
	equipment_id?: number | null;
}

export interface ChainInput {
	name: string;
	description?: string;
	components: ChainComponent[];
	listening_context?: string;
	icon?: string | null;
}

export interface EquipmentInput {
	name: string;
	type: string;
	brand?: string;
	model?: string;
	notes?: string;
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

export interface Equipment {
	id: number;
	name: string;
	equipment_type: string;
	brand: string | null;
	model: string | null;
	total_hours: number;
	first_used: number | null;
	last_used: number | null;
}

/** Live playback state. Field names mirror `Scrobble` so `qualityLabel()` works on it. */
export interface NowPlayingTrack {
	title: string;
	artist: string;
	album: string | null;
	duration: number | null;
	progress: number;
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
	loved: boolean;
	revisions: number;
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
	kind?: '' | 'recording' | 'release' | 'artist' | 'listen';
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

export interface ArtistView {
	id: number;
	name: string;
	mbid: string | null;
	/** Portrait: from the metadata chain, from the user's own library through
	 *  the proxy, or set by hand. Null means the lettered tile. */
	image_url: string | null;
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
	started_at: number;
	ended_at: number | null;
	track_count: number;
	total_duration: number;
	avg_quality_score: number | null;
	listening_context: string | null;
	chain_id: number | null;
	chain_name: string | null;
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
export interface PhysicalTrack {
	side: string;
	position: number;
	title: string;
	artist?: string | null;
	duration_secs?: number | null;
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

/** A record or tape on the shelf. Counters are derived, never accumulated. */
export interface PhysicalRelease {
	id: number;
	kind: 'vinyl' | 'cassette';
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
	play_count: number;
	played_secs: number;
	last_played: number | null;
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
	}): Promise<{ scrobbles: Scrobble[]; count: number }> {
		const query = new URLSearchParams();
		if (params?.limit) query.set('limit', String(params.limit));
		if (params?.offset) query.set('offset', String(params.offset));
		if (params?.artist) query.set('artist', params.artist);
		if (params?.album) query.set('album', params.album);
		if (params?.track) query.set('track', params.track);
		if (params?.after) query.set('after', String(params.after));
		if (params?.before) query.set('before', String(params.before));
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
	async addPressing(body: Partial<ReleaseCandidate> & { kind: string; tracks: PhysicalTrack[] }) {
		return this.post<{ id: number }>('/api/v1/physical', body);
	}
	async setPressingTracks(id: number, tracks: PhysicalTrack[]) {
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
		}
	): Promise<PlayResult> {
		return this.post(`/api/v1/physical/${id}/play`, body);
	}
	async equipmentWear(id: number): Promise<{ hours: number; sides_played: number }> {
		return this.get(`/api/v1/physical/equipment/${id}/wear`);
	}
	/** Set or clear an artist portrait by hand. Empty clears it. */
	async setArtistImage(artistRowId: number, url: string): Promise<{ image_url: string | null }> {
		return this.request('PUT', `/api/v1/artists/img/${artistRowId}`, { url });
	}
	/** Set or clear an album cover by hand — takes the *entity* id, the same one
	 *  the album page is addressed by. Empty restores the cover derived from the
	 *  listens. */
	async setAlbumCover(entityId: number, url: string): Promise<{ artwork_url: string | null }> {
		return this.request('PUT', `/api/v1/albums/${entityId}/cover`, { url });
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
	async rebuildSessions(gap_seconds?: number): Promise<{ sessions: number; listens: number }> {
		return this.post('/api/v1/sessions/rebuild', gap_seconds ? { gap_seconds } : {});
	}
	async updateSession(id: number, data: { chain_id?: number | null; title?: string | null }): Promise<{ session: Session }> {
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

	// ── Album & artist pages ──
	async getAlbum(id: number): Promise<AlbumView> { return this.get(`/api/v1/albums/${id}`); }
	async getArtist(id: number): Promise<ArtistView> { return this.get(`/api/v1/artists/${id}`); }
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
	async bulkSetChain(ids: number[], signal_chain_id: number | null): Promise<{ updated: number }> {
		return this.post('/api/v1/scrobbles/bulk-chain', { ids, signal_chain_id });
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
	async noteFor(target: NoteTarget): Promise<{ note: Note | null; target: { entity?: Entity | null; listen?: NoteListen | null }; loved: boolean }> {
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

	// ── Tokens ──
	async getTokens(): Promise<{ tokens: TokenInfo[] }> { return this.get('/admin/tokens'); }
	async createToken(name: string, opts?: { scopes?: string; default_chain_id?: number | null }): Promise<{ token: string; name: string; user_id: number; message: string }> {
		return this.post('/admin/tokens', {
			name,
			scopes: opts?.scopes ?? 'submit',
			default_chain_id: opts?.default_chain_id ?? null
		});
	}
	// ── Device-code pairing ──
	/** What the user is about to approve. */
	async describePairing(userCode: string): Promise<{
		client_name: string;
		scopes: string;
		can_read: boolean;
		approved: boolean;
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
	async updateToken(id: number, data: { name?: string; default_chain_id?: number | null }): Promise<void> {
		await this.patch(`/admin/tokens/${id}`, data);
	}
	async revokeToken(id: number): Promise<void> { await this.del(`/admin/tokens/${id}`); }
	/** Re-issue a token you've lost. Same row, new secret — clients must be
	 *  re-pasted, which is why it's separate from `updateToken`. */
	async rotateToken(id: number): Promise<{ token: string; message: string }> {
		return this.post(`/admin/tokens/${id}/rotate`, {});
	}
}

export const api = new TapedeckAPI();
