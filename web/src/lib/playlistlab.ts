import type { LabTrack, LabDim } from './api';

/**
 * The three ways the Playlist Lab grows a list, and the distance they all use.
 *
 * The algorithms are the design's own (`temp/Tapedeck-PlaylistLabs.dc.html`) and
 * they were fine — it was only the *vectors* underneath them that were faked,
 * hashed out of the artist's name so the mockup's plot would look plausible.
 * The server supplies real ones now; see `src/playlist_lab.rs` for what they
 * are and what they are not.
 *
 * This runs in the browser rather than on the server because the design drives
 * it from sliders: a round trip per slider tick would make the screen feel
 * broken, and the phase-space plot needs the whole pool loaded anyway to draw
 * its background dots. The part that is easy to get wrong — the map join, the
 * timezone-correct hour bucketing, the normalisation — is all server-side and
 * tested. What is here is geometry over an array of ten floats.
 */

export type Mode = 'similar' | 'crawl' | 'coaster';

/** How many Sound axes lead the vector. Must match `Group::Sound` in Rust. */
export const SOUND_DIMS = 6;

export interface Params {
	length: number;
	/** similar: 0 = tightest matches, 1 = adventurous. */
	diversity: number;
	/** crawl: 0 = smooth small hops, 1 = bold leaps. */
	step: number;
	/** coaster: 0 = gentle rolls, 1 = wild swings. */
	amp: number;
	/** Which axis the rollercoaster rides. */
	rideDim: number;
	/**
	 * 0 = judge only by how you listen, 1 = only by how it sounds.
	 *
	 * This exists because the space deliberately mixes two kinds of axis, and
	 * without a weight the four Habit axes quietly turn "similar songs" into
	 * "songs I play at the same hour". Defaulted toward sound.
	 */
	soundWeight: number;
	/** Reshuffles the rollercoaster's random trajectory. */
	seedN: number;
}

export const DEFAULTS: Params = {
	length: 12,
	diversity: 0.35,
	step: 0.4,
	amp: 0.5,
	rideDim: 0,
	soundWeight: 0.75,
	seedN: 1
};

/** Per-axis multipliers for the current sound/habit balance. */
export function weights(soundWeight: number, n: number): number[] {
	// Both halves keep some presence at the extremes — a slider at 0 that
	// erased six axes outright would make the plot jump rather than shift, and
	// "only habits" is not a thing anyone asked for.
	const s = 0.25 + 1.5 * soundWeight;
	const h = 0.25 + 1.5 * (1 - soundWeight);
	return Array.from({ length: n }, (_, i) => (i < SOUND_DIMS ? s : h));
}

/** Weighted Euclidean distance, optionally ignoring the first `from` axes. */
export function dist(a: number[], b: number[], w: number[], from = 0): number {
	let s = 0;
	for (let i = from; i < a.length; i++) {
		const d = (a[i] - b[i]) * w[i];
		s += d * d;
	}
	return Math.sqrt(s);
}

/** Deterministic 0–1 from a string, so "adventurous" is stable across renders. */
function hash(str: string): number {
	let h = 2166136261;
	for (let i = 0; i < str.length; i++) {
		h ^= str.charCodeAt(i);
		h = Math.imul(h, 16777619);
	}
	return ((h >>> 0) % 100000) / 100000;
}

/** Small seeded PRNG (mulberry32), so a trajectory can be reshuffled and kept. */
function rng(seed: number): () => number {
	let a = seed;
	return function () {
		a |= 0;
		a = (a + 0x6d2b79f5) | 0;
		let t = Math.imul(a ^ (a >>> 15), 1 | a);
		t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

export interface Step {
	track: LabTrack;
	index: number;
	isSeed: boolean;
	/** What to show beside the row — a match %, a hop size, a height. */
	metric: string;
}

export interface Result {
	steps: Step[];
	/** Seed centroid, for the "similar" rings on the plot. */
	centre: number[] | null;
	/** True when the plot should draw a path through the steps. */
	path: boolean;
	/** How many seeds this mode still needs. 0 when it can run. */
	need: number;
}

const EMPTY = (need: number): Result => ({ steps: [], centre: null, path: false, need });

/**
 * Build a playlist.
 *
 * `pool` is every candidate, `seeds` are indices into it. Seeds always appear in
 * the result — they are the user's own choice and dropping one would be the
 * generator overruling them.
 */
export function generate(
	pool: LabTrack[],
	seeds: number[],
	mode: Mode,
	p: Params
): Result {
	const nDims = pool[0]?.vector.length ?? 0;
	if (!nDims) return EMPTY(1);
	const w = weights(p.soundWeight, nDims);

	const chosen = seeds.filter((i) => pool[i]).map((i) => ({ t: pool[i], i }));
	const isSeed = new Set(chosen.map((c) => c.i));
	const cand = pool.map((t, i) => ({ t, i })).filter((c) => !isSeed.has(c.i));

	if (mode === 'similar') {
		if (!chosen.length) return EMPTY(1);
		const centre = Array.from({ length: nDims }, (_, d) =>
			chosen.reduce((s, c) => s + c.t.vector[d], 0) / chosen.length
		);
		const scored = cand.map((c) => {
			const d = dist(centre, c.t.vector, w);
			// Diversity blends the real distance with a stable per-track jitter,
			// which loosens the ranking without making it different every time
			// the slider is nudged back to where it was.
			const noise = hash(c.t.title + c.t.artist);
			return { c, d, eff: d * (1 - 0.55 * p.diversity) + p.diversity * 0.95 * noise };
		});
		scored.sort((a, b) => a.eff - b.eff);
		const picks = scored.slice(0, Math.max(0, p.length - chosen.length));

		// The scale is the pool's own spread, not a constant: a percentage
		// against an invented maximum would be a confident-looking way of
		// saying nothing.
		const far = scored.length ? scored[scored.length - 1].d || 1 : 1;
		const steps: Step[] = [
			...chosen.map((c) => ({ track: c.t, index: c.i, isSeed: true, metric: 'seed' })),
			...picks.map((s) => ({
				track: s.c.t,
				index: s.c.i,
				isSeed: false,
				metric: `${Math.max(1, Math.round(100 - (s.d / far) * 100))}%`
			}))
		];
		return { steps, centre, path: false, need: 0 };
	}

	if (mode === 'crawl') {
		if (chosen.length < 2) return EMPTY(2 - chosen.length);
		const used = new Set(chosen.map((c) => c.i));
		const segs = chosen.length - 1;
		const inter = Math.max(0, p.length - chosen.length);
		const steps: Step[] = [
			{ track: chosen[0].t, index: chosen[0].i, isSeed: true, metric: 'start' }
		];
		let cur = chosen[0].t.vector;

		for (let si = 0; si < segs; si++) {
			const B = chosen[si + 1];
			const slots = Math.floor(inter / segs) + (si < inter % segs ? 1 : 0);
			for (let k = 0; k < slots; k++) {
				const dCB = dist(cur, B.t.vector, w);
				let best: { t: LabTrack; i: number } | null = null;
				let bs = Infinity;
				for (const c of cand) {
					if (used.has(c.i)) continue;
					const dc = dist(cur, c.t.vector, w);
					const db = dist(c.t.vector, B.t.vector, w);
					// Must actually get closer to the destination, or the walk
					// wanders and the "path" on the plot means nothing.
					if (db >= dCB - 1e-6) continue;
					const sc = dc * (1 - p.step * 0.55) + db * (0.35 + p.step * 0.85);
					if (sc < bs) {
						bs = sc;
						best = c;
					}
				}
				if (!best) break;
				const hop = dist(cur, best.t.vector, w);
				used.add(best.i);
				steps.push({
					track: best.t,
					index: best.i,
					isSeed: false,
					metric: `+${hop.toFixed(2)}`
				});
				cur = best.t.vector;
			}
			used.add(B.i);
			steps.push({ track: B.t, index: B.i, isSeed: true, metric: 'seed' });
			cur = B.t.vector;
		}
		return { steps, centre: null, path: true, need: 0 };
	}

	// Rollercoaster.
	if (!chosen.length) return EMPTY(1);
	const rand = rng(p.seedN * 2654435761);
	const used = new Set([chosen[0].i]);
	const ride = Math.min(Math.max(0, p.rideDim), nDims - 1);
	const steps: Step[] = [
		{
			track: chosen[0].t,
			index: chosen[0].i,
			isSeed: true,
			metric: chosen[0].t.vector[ride].toFixed(2)
		}
	];
	let cur = chosen[0].t.vector;
	const n = Math.max(0, p.length - 1);
	const peaks = 2 + Math.round(p.amp * 3);
	const phase = rand() * Math.PI * 2;

	for (let k = 0; k < n; k++) {
		const t = (k + 1) / n;
		const wob = Math.sin(t * Math.PI * peaks + phase);
		const target = Math.max(
			0.05,
			Math.min(0.95, 0.5 + wob * (0.16 + p.amp * 0.34) + (rand() - 0.5) * 0.12)
		);
		let best: { t: LabTrack; i: number } | null = null;
		let bs = Infinity;
		for (const c of cand) {
			if (used.has(c.i)) continue;
			const de = Math.abs(c.t.vector[ride] - target);
			// The ride axis drives the shape; every *other* axis still pulls
			// toward the previous track, so a rollercoaster is a journey rather
			// than a random walk that happens to go up and down.
			let dr = 0;
			for (let i = 0; i < nDims; i++) {
				if (i === ride) continue;
				const d = (c.t.vector[i] - cur[i]) * w[i];
				dr += d * d;
			}
			dr = Math.sqrt(dr);
			const sc = de * 1.5 + dr * (1 - p.amp * 0.6) + rand() * 0.18 * p.amp;
			if (sc < bs) {
				bs = sc;
				best = c;
			}
		}
		if (!best) break;
		used.add(best.i);
		steps.push({
			track: best.t,
			index: best.i,
			isSeed: false,
			metric: best.t.vector[ride].toFixed(2)
		});
		cur = best.t.vector;
	}
	return { steps, centre: null, path: true, need: 0 };
}

/** The playlist as a file. */
export function buildExport(
	steps: Step[],
	name: string,
	fmt: 'm3u' | 'json' | 'text'
): string {
	const nm = name.trim() || 'Untitled playlist';
	if (fmt === 'json') {
		return JSON.stringify(
			{
				name: nm,
				count: steps.length,
				// Named so nobody reads this as an audio analysis — the same
				// thing the API's own `granularity` field says.
				generated_by: 'Tapedeck Playlist Lab (Every Noise genre space + listening habits)',
				tracks: steps.map((s) => ({
					title: s.track.title,
					artist: s.track.artist,
					album: s.track.album,
					plays: s.track.plays
				}))
			},
			null,
			2
		);
	}
	if (fmt === 'text') {
		return nm + '\n' + steps.map((s, i) => `${i + 1}. ${s.track.title} — ${s.track.artist}`).join('\n');
	}
	// Two honest compromises in here.
	//
	// `-1` for duration: Tapedeck knows a running time for some listens and not
	// others, and a guess would be worse than the "unknown" every player already
	// understands.
	//
	// The URI line is `Artist - Title` with **no extension**. An M3U entry
	// without a URI is not an entry at all — players skip it — so one has to be
	// written, but Tapedeck does not know where the files are and the design's
	// mockup invented `Artist - Title.flac`, which claims a format and a layout
	// that are very unlikely to be right. A bare name is at least what it looks
	// like: a line for a human or a resolver to match on. Anyone wanting a
	// playlist that actually plays should use "Send to…", which resolves against
	// the real library.
	return (
		'#EXTM3U\n#PLAYLIST:' +
		nm +
		'\n' +
		steps
			.map((s) => `#EXTINF:-1,${s.track.artist} - ${s.track.title}\n${s.track.artist} - ${s.track.title}`)
			.join('\n') +
		'\n'
	);
}

/** A short label for an axis, for the plot's edges and the ride picker. */
export function axisName(dims: LabDim[], i: number): string {
	return dims[i]?.label ?? `Axis ${i + 1}`;
}
