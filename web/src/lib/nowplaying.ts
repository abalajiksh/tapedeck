import { writable } from 'svelte/store';
import { browser } from '$app/environment';
import { api, type NowPlayingTrack } from './api';

export interface DeckState {
	playing: boolean;
	track: NowPlayingTrack | null;
	/** Seconds into the track. Ticked locally between polls so the bar moves smoothly. */
	position: number;
}

const EMPTY: DeckState = { playing: false, track: null, position: 0 };

export const deck = writable<DeckState>(EMPTY);

// The sidebar card and the dashboard deck both want this, so the poll is
// reference-counted — two subscribers still means one request every POLL_MS.
const POLL_MS = 10_000;
let watchers = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let tickTimer: ReturnType<typeof setInterval> | null = null;

async function poll() {
	try {
		const s = await api.getNowPlaying();
		deck.set(
			s.playing && s.track
				? { playing: true, track: s.track, position: s.position ?? 0 }
				: EMPTY
		);
	} catch {
		// A failed poll (offline, session expired) just leaves the deck as-is
		// rather than flickering it empty; the next poll corrects it.
	}
}

function tick() {
	deck.update((d) => {
		if (!d.playing || !d.track) return d;
		const next = d.position + 1;
		// Don't run past the end — the next poll decides whether it's still playing.
		const max = d.track.duration ?? Infinity;
		return { ...d, position: Math.min(next, max) };
	});
}

/**
 * Keep `deck` fresh for as long as the caller is mounted.
 * Returns the teardown to hand back from `onMount`.
 */
export function watchNowPlaying(): () => void {
	if (!browser) return () => {};

	if (watchers === 0) {
		poll();
		pollTimer = setInterval(poll, POLL_MS);
		tickTimer = setInterval(tick, 1000);
	}
	watchers++;

	return () => {
		watchers = Math.max(0, watchers - 1);
		if (watchers === 0) {
			if (pollTimer) clearInterval(pollTimer);
			if (tickTimer) clearInterval(tickTimer);
			pollTimer = tickTimer = null;
			deck.set(EMPTY);
		}
	};
}
