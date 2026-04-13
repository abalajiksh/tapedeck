/** Tapedeck API client — talks to the Axum backend. */

const BASE = '';  // Same origin in production (embedded), proxied in dev

export interface Scrobble {
	id: number;
	title: string;
	artist: string;
	album: string | null;
	timestamp: number;
	duration: number | null;
	status: string;
	// Quality
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
	// Context
	listening_context: string | null;
	submission_client: string | null;
	// MusicBrainz
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

class TapedeckAPI {
	private token: string = '';

	setToken(token: string) {
		this.token = token;
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem('tapedeck_token', token);
		}
	}

	getToken(): string {
		if (!this.token && typeof localStorage !== 'undefined') {
			this.token = localStorage.getItem('tapedeck_token') ?? '';
		}
		return this.token;
	}

	private headers(): Record<string, string> {
		const h: Record<string, string> = { 'Content-Type': 'application/json' };
		const t = this.getToken();
		if (t) h['Authorization'] = `Token ${t}`;
		return h;
	}

	private async get<T>(path: string): Promise<T> {
		const res = await fetch(`${BASE}${path}`, { headers: this.headers() });
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}

	private async post<T>(path: string, body: unknown): Promise<T> {
		const res = await fetch(`${BASE}${path}`, {
			method: 'POST',
			headers: this.headers(),
			body: JSON.stringify(body),
		});
		if (!res.ok) throw new Error(`API ${res.status}: ${await res.text()}`);
		return res.json();
	}

	// Health
	async health(): Promise<HealthStatus> {
		return this.get('/health');
	}

	// Scrobbles
	async getScrobbles(params?: {
		limit?: number;
		offset?: number;
		artist?: string;
		album?: string;
		after?: number;
		before?: number;
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

	// Dashboard stats
	async getDashboardStats(): Promise<DashboardStats> {
		return this.get('/api/v1/stats/dashboard');
	}

	// Chains
	async getChains(): Promise<{ chains: SignalChain[] }> {
		return this.get('/api/v1/chains');
	}

	async getChain(id: number): Promise<SignalChain> {
		return this.get(`/api/v1/chains/${id}`);
	}

	async createChain(data: {
		name: string;
		description?: string;
		components: ChainComponent[];
		listening_context?: string;
	}): Promise<{ id: number }> {
		return this.post('/api/v1/chains', data);
	}

	// Devices
	async getDevices(): Promise<{ devices: Device[] }> {
		return this.get('/api/v1/devices');
	}

	// Equipment
	async getEquipment(): Promise<{ equipment: Equipment[] }> {
		return this.get('/api/v1/equipment');
	}

	// Users
	async getUsers(): Promise<{ users: Array<{ id: number; username: string; display_name: string | null; created_at: number }> }> {
		return this.get('/admin/users');
	}

	async getTokens(): Promise<{ tokens: Array<{ id: number; name: string; scopes: string; created_at: number; last_used_at: number | null }> }> {
		return this.get('/admin/tokens');
	}
}

export const api = new TapedeckAPI();
