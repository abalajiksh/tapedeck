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
	lossless_pct: number;
	top_artist: string;
	top_artist_count: number;
	unique_artists: number;
	unique_albums: number;
	unique_tracks: number;
	total_hours: number;
	avg_quality: number;
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

export interface HealthStatus {
	status: string;
	service: string;
	version: string;
}

export interface AuthStatus {
	needs_setup: boolean;
	authenticated: boolean;
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

	// ── Chains ──
	async getChains(): Promise<{ chains: SignalChain[] }> { return this.get('/api/v1/chains'); }
	async getChain(id: number): Promise<SignalChain> { return this.get(`/api/v1/chains/${id}`); }
	async createChain(data: { name: string; description?: string; components: ChainComponent[]; listening_context?: string }): Promise<{ id: number }> {
		return this.post('/api/v1/chains', data);
	}

	// ── Devices & Equipment ──
	async getDevices(): Promise<{ devices: Device[] }> { return this.get('/api/v1/devices'); }
	async getEquipment(): Promise<{ equipment: Equipment[] }> { return this.get('/api/v1/equipment'); }

	// ── Users ──
	async getUsers(): Promise<{ users: Array<{ id: number; username: string; display_name: string | null; role: string; created_at: number }> }> {
		return this.get('/admin/users');
	}

	// ── Tokens ──
	async getTokens(): Promise<{ tokens: TokenInfo[] }> { return this.get('/admin/tokens'); }
	async createToken(name: string, scopes?: string): Promise<{ token: string; name: string; user_id: number; message: string }> {
		return this.post('/admin/tokens', { name, scopes: scopes ?? 'submit' });
	}
	async revokeToken(id: number): Promise<void> { await this.del(`/admin/tokens/${id}`); }
}

export const api = new TapedeckAPI();
