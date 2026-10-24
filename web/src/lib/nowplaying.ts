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
//
// 3s rather than 10s: the server reads an in-memory map and returns a few
// hundred bytes, so this is close to free, and it was the last stretch of the
// delay between pressing play and the deck showing it.
const POLL_MS = 3_000;

// How far the server's playhead may differ from our own count before we snap to
// it. The server already refuses to move backwards on a stale report, but the
// two clocks still disagree by a second either way — and snapping every 3s to
// settle a one-second argument is visible as a stutter in a bar that is
// otherwise moving smoothly. Anything larger than this is a real seek, a track
// change or a long stall, and should land immediately.
const DRIFT_TOLERANCE_SECS = 2;

let watchers = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let tickTimer: ReturnType<typeof setInterval> | null = null;

async function poll() {
	try {
		const s = await api.getNowPlaying();
		if (!s.playing || !s.track) {
			deck.set(EMPTY);
			return;
		}
		const track = s.track;
		const reported = s.position ?? 0;
		deck.update((d) => {
			const sameTrack =
				d.playing && d.track?.title === track.title && d.track?.artist === track.artist;
			// Keep our own count while the server agrees to within a second or
			// two — see DRIFT_TOLERANCE_SECS. A different track always takes the
			// server's number, since ours belongs to the previous one, and so does
			// a paused deck: there is no motion to keep smooth, only a place to stop.
			const position =
				sameTrack && !track.paused && Math.abs(reported - d.position) <= DRIFT_TOLERANCE_SECS
					? d.position
					: reported;
			return { playing: true, track, position };
		});
	} catch {
		// A failed poll (offline, session expired) just leaves the deck as-is
		// rather than flickering it empty; the next poll corrects it.
	}
}

function tick() {
	deck.update((d) => {
		// A paused deck holds still, as the server does; counting on is what
		// walked it forward until the next poll snapped it back.
		if (!d.playing || !d.track || d.track.paused) return d;
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
