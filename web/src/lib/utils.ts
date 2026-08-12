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

/**
 * Format duration in seconds to mm:ss or h:mm:ss.
 * Zero is a real value here (a track that just started), so only missing or
 * nonsensical input renders as an em dash.
 */
export function formatDuration(seconds: number | null | undefined): string {
	if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return '—';
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	const s = seconds % 60;
	if (h > 0) return `${h}:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}`;
	return `${m}:${String(s).padStart(2, '0')}`;
}

/**
 * A unix timestamp as `YYYY-MM-DD` for an `<input type="date">`.
 *
 * Built from the **local** calendar parts rather than `toISOString().slice(0,10)`,
 * which converts to UTC first — so a purchase stored at 23:00 CET would show as
 * the day before, and saving without touching the field would then move it.
 */
export function toDateInput(ts: number | null | undefined): string {
	if (ts == null) return '';
	const d = new Date(ts * 1000);
	const p = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** The other direction. Midday local, not midnight: a date with no time in it
 *  should not be able to land on the previous day under any timezone. */
export function fromDateInput(value: string): number | null {
	if (!value) return null;
	const [y, m, d] = value.split('-').map(Number);
	if (!y || !m || !d) return null;
	return Math.floor(new Date(y, m - 1, d, 12, 0, 0).getTime() / 1000);
}

/**
 * How long something has been owned, in the coarsest unit that still says
 * something — "3 years" rather than "3 years, 2 months, 11 days", which is
 * precision nobody asked for about a date most people are estimating anyway.
 */
export function ownedFor(purchasedAt: number | null | undefined): string {
	if (purchasedAt == null) return '';
	const days = Math.floor((Date.now() / 1000 - purchasedAt) / 86400);
	if (days < 0) return 'not yet';
	if (days < 31) return days <= 1 ? 'new' : `${days} days`;
	const months = Math.floor(days / 30.44);
	if (months < 24) return `${months} month${months === 1 ? '' : 's'}`;
	const years = Math.floor(days / 365.25);
	return `${years} years`;
}

/** Format hours to a human-friendly string */
export function formatHours(hours: number): string {
	if (hours < 1) return `${Math.round(hours * 60)}min`;
	if (hours < 100) return `${hours.toFixed(1)}h`;
	return `${Math.round(hours)}h`;
}

/**
 * The four media the shelf writes, exactly as `physical.rs::medium_label()`
 * spells them. Matching against a fixed set rather than trusting the field is
 * the whole safety of this: `submission_client` is free text every other source
 * fills with whatever the client calls itself, and nothing is going to name
 * itself "Vinyl".
 */
const MEDIA = ['Vinyl', 'Cassette', 'CD', 'SACD'] as const;

/**
 * The badge for a listen that came off an object rather than a file.
 *
 * A record has no codec and never will, so `qualityLabel` returns nothing for
 * one and the badge slot sat empty — a side of vinyl and an unlabelled import
 * looked identical in the history. This says what the thing *was*. It carries
 * no tier on purpose: a medium is a fact, not a position on a scale, which is
 * the same reason it is styled apart from the fidelity tags.
 *
 * Reads both spellings because the two paths differ and always have: a stored
 * listen files the medium under `submission_client`, while the deck's
 * now-playing entry carries it as `listening_context`.
 */
export function mediumLabel(x: {
	submission_client?: string | null;
	listening_context?: string | null;
}): string | null {
	const claimed = (x.submission_client ?? x.listening_context ?? '').trim();
	return MEDIA.find((m) => m.toLowerCase() === claimed.toLowerCase()) ?? null;
}

/** Build a quality badge label from scrobble data */
export function qualityLabel(scrobble: {
	format_type?: string | null;
	codec?: string | null;
	bit_depth?: number | null;
	sample_rate?: number | null;
	dsd_multiplier?: number | null;
	dsd_to_pcm_converted?: boolean | null;
	delivery_codec?: string | null;
	is_lossless?: boolean | null;
}): { text: string; tier: 'dsd' | 'lossless' | 'lossy' | 'bt' } {
	if (scrobble.format_type === 'dsd' && scrobble.dsd_multiplier) {
		// A DSD file decoded to PCM on the way out is not native DSD playback,
		// and on a chain built around a DSD-capable DAC that difference is the
		// whole point — so it is on the badge rather than buried in a detail
		// panel. The tier stays `dsd`: it is still a DSD source.
		const suffix = scrobble.dsd_to_pcm_converted ? '→PCM' : '';
		return { text: `DSD${scrobble.dsd_multiplier}${suffix}`, tier: 'dsd' };
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

/** Map a quality tier to the DS tag class used for badges. */
export function tierTagClass(tier: 'dsd' | 'lossless' | 'lossy' | 'bt'): string {
	return 'tag ' + ({ lossless: 'tag-accent-2', dsd: 'tag-accent', lossy: 'tag-neutral', bt: 'tag-outline' }[tier] ?? 'tag-neutral');
}

/** Deterministic gradient cover for a track, keyed by artist/title initial. */
const COVERS: [string, string][] = [
	['#c67139', '#e07a55'], ['#7a8a5e', '#aebf92'], ['#b2622d', '#f6a06b'],
	['#56633f', '#8fa073'], ['#8c491a', '#d67f48'], ['#645c50', '#a19786']
];
export function coverStyle(seed: string): string {
	let h = 0;
	for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
	const [a, b] = COVERS[h % COVERS.length];
	return `background:linear-gradient(135deg,${a},${b});`;
}

/** Cover Art Archive URL from caa_id and release MBID */
export function coverArtUrl(
	caaId: number | null | undefined,
	releaseId: string | null | undefined,
	size: 250 | 500 | 1200 = 250
): string | null {
	// The backfill records -1 for "looked up, nothing there", so it stops
	// retrying; treat any non-positive id as no artwork.
	if (!caaId || caaId <= 0 || !releaseId) return null;
	return `https://coverartarchive.org/release/${releaseId}/${caaId}-${size}.jpg`;
}
