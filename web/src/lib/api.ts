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
	is_lossless: boolean | null;
	dsd_rate: number | null;
	dsd_multiplier: number | null;
	delivery_codec: string | null;
	quality_score: number | null;
	chain_id: number | null;
	listening_context: string | null;
	submission_client: string | null;
	mbid_recording: string | null;
	mbid_release: string | null;
	caa_id: number | null;
	caa_release_mbid: string | null;
	/** Resolved cover image from whichever provider answered. */
	artwork_url: string | null;
}

export interface DashboardStats {
	today: number;
	this_week: number;
	total: number;
	today_hours: number;
	week_hours: number;
	lossless_pct: number;
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
	};
	top_artists: { name: string; plays: number; hours: number; image_url: string | null }[];
	top_albums: { name: string; artist: string; plays: number; hours: number }[];
	top_tracks: { title: string; artist: string; album: string | null; plays: number; hours: number }[];
	fidelity: { name: string; plays: number; pct: number }[];
	genres: { name: string; pct: number }[];
	languages: { name: string; pct: number }[];
	regions: { name: string; pct: number }[];
	decades: { name: string; plays: number; pct: number }[];
	heatmap: number[][]; // 7 days × 24 hours
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

export interface SanitizePreview {
	/** Albums whose listens disagree about which release they are. */
	mixed_albums: MixedAlbum[];
	/** `Artist feat. X` strings that fragment an album or artist page. */
	featured_artists: { artist: string; primary: string; plays: number }[];
	/** One artist spelled several ways ("A.R. Rahman" / "A. R. Rahman"). */
	artist_variants: { primary: string; primary_plays: number; variants: [string, number][] }[];
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

	async getRangedStats(range: StatsRange = 'all'): Promise<RangedStats> {
		return this.get(`/api/v1/stats?range=${range}`);
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
		body: { side: string; started_at?: number; equipment_id?: number | null; chain_id?: number | null }
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

	async getDiscogsSetting(): Promise<{ configured: boolean; from_env: boolean; stored: boolean }> {
		return this.get('/api/v1/physical/settings/discogs');
	}
	async setDiscogsSetting(token: string): Promise<{ configured: boolean }> {
		return this.request('PUT', '/api/v1/physical/settings/discogs', { token });
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
	async getChart(kind: ChartKind, range: ChartRange, limit = 100): Promise<{ kind: ChartKind; range: string; rows: ChartRow[] }> {
		return this.get(`/api/v1/charts?kind=${kind}&range=${range}&limit=${limit}`);
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
	async getNotes(): Promise<{ notes: Note[] }> { return this.get('/api/v1/notes'); }
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
	async getLoves(): Promise<{ loves: { entity: Entity; loved_at: number }[] }> {
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
