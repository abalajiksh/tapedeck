/** Format a Unix timestamp to a relative time string (e.g. "3 min ago") */
export function timeAgo(timestamp: number): string {
	const now = Math.floor(Date.now() / 1000);
	const diff = now - timestamp;

	if (diff < 60) return 'just now';
	if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
	if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
	if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;

	const date = new Date(timestamp * 1000);
	return date.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
}

/** Format a Unix timestamp to a full date-time string */
export function formatDateTime(timestamp: number): string {
	const date = new Date(timestamp * 1000);
	return date.toLocaleString('en-GB', {
		day: 'numeric',
		month: 'short',
		year: 'numeric',
		hour: '2-digit',
		minute: '2-digit',
	});
}

/** Format duration in seconds to mm:ss or h:mm:ss */
export function formatDuration(seconds: number | null | undefined): string {
	if (!seconds) return '—';
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	const s = seconds % 60;
	if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
	return `${m}:${String(s).padStart(2, '0')}`;
}

/** Format hours to a human-friendly string */
export function formatHours(hours: number): string {
	if (hours < 1) return `${Math.round(hours * 60)}min`;
	if (hours < 100) return `${hours.toFixed(1)}h`;
	return `${Math.round(hours)}h`;
}

/** Build a quality badge label from scrobble data */
export function qualityLabel(scrobble: {
	format_type?: string | null;
	codec?: string | null;
	bit_depth?: number | null;
	sample_rate?: number | null;
	dsd_multiplier?: number | null;
	delivery_codec?: string | null;
	is_lossless?: boolean | null;
}): { text: string; tier: 'dsd' | 'lossless' | 'lossy' | 'bt' } {
	if (scrobble.format_type === 'dsd' && scrobble.dsd_multiplier) {
		return { text: `DSD${scrobble.dsd_multiplier}`, tier: 'dsd' };
	}

	if (scrobble.delivery_codec) {
		const dc = scrobble.delivery_codec.toUpperCase();
		return { text: dc, tier: 'bt' };
	}

	if (scrobble.is_lossless && scrobble.codec) {
		const codec = scrobble.codec.toUpperCase();
		const bd = scrobble.bit_depth ?? 16;
		const sr = scrobble.sample_rate ? Math.round(scrobble.sample_rate / 1000) : 44.1;
		return { text: `${codec} ${bd}/${sr}`, tier: 'lossless' };
	}

	if (scrobble.codec) {
		return { text: scrobble.codec.toUpperCase(), tier: 'lossy' };
	}

	return { text: '', tier: 'lossy' };
}

/** Cover Art Archive URL from caa_id and release MBID */
export function coverArtUrl(
	caaId: number | null | undefined,
	releaseId: string | null | undefined,
	size: 250 | 500 | 1200 = 250
): string | null {
	if (!caaId || !releaseId) return null;
	return `https://coverartarchive.org/release/${releaseId}/${caaId}-${size}.jpg`;
}
