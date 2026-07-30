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
	top_artists: { name: string; plays: number; hours: number }[];
	top_albums: { name: string; artist: string; plays: number; hours: number }[];
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
	kind: string;
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
	Omit<UserSource, 'kind' | 'has_token' | 'remote_account' | 'last_polled_at' | 'last_error' | 'updated_at'>
> & { token?: string; username?: string; password?: string };

export interface SourceTestResult {
	ok: boolean;
	libraries?: string[];
	account?: string | null;
	account_error?: string | null;
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

export interface TokenInfo {
	id: number;
	name: string;
	scopes: string;
	created_at: number;
	last_used_at: number | null;
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

	async putSource(kind: string, body: UserSourceUpdate): Promise<UserSource> {
		return this.request('PUT', `/api/v1/sources/${kind}`, body);
	}

	async deleteSource(kind: string): Promise<void> {
		return this.delNoBody(`/api/v1/sources/${kind}`);
	}

	async testSource(
		kind: string,
		body: { url?: string; token?: string; username?: string; password?: string }
	): Promise<SourceTestResult> {
		return this.post(`/api/v1/sources/${kind}/test`, body);
	}

	// ── Scrobbles ──
	async getScrobbles(params?: {
		limit?: number; offset?: number; artist?: string;
		album?: string; after?: number; before?: number;
	}): Promise<{ scrobbles: Scrobble[]; count: number }> {
		const query = new URLSearchParams();
		if (params?.limit) query.set('limit', String(params.limit));
		if (params?.offset) query.set('offset', String(params.offset));
		if (params?.artist) query.set('artist', params.artist);
		if (params?.album) query.set('album', params.album);
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
	async getDevices(): Promise<{ devices: Device[] }> { return this.get('/api/v1/devices'); }
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
	async createToken(name: string, scopes?: string): Promise<{ token: string; name: string; user_id: number; message: string }> {
		return this.post('/admin/tokens', { name, scopes: scopes ?? 'submit' });
	}
	async revokeToken(id: number): Promise<void> { await this.del(`/admin/tokens/${id}`); }
}

export const api = new TapedeckAPI();
