import { writable } from 'svelte/store';
import { browser } from '$app/environment';
import { api, type NowPlayingTrack } from './api';

export interface DeckState {
	playing: boolean;
	track: NowPlayingTrack | null;
	/** Seconds into the track, **with a fraction** — a lyric line can turn
	 *  over between two whole seconds. Floor it before printing it. */
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

// ── The clock ──
//
// The deck used to add 1 to a whole-second count on a 1 s `setInterval` and
// snap to the server whenever the two disagreed by more than 2 s. A timer is
// not a clock — it fires late, and much later in a background tab — so the
// count slipped about a second behind every twenty and then jumped 3–5 s to
// catch up (measured against Plexamp, 2026-10-24). Half of the back-and-forth.
//
// Now the position is a function of `performance.now()`: an anchor (where the
// track was, and when) plus the time since. A poll does not overwrite it; it
// re-anchors where the clock already is and spreads the difference over the
// next few seconds, so the deck speeds up or slows down slightly instead of
// jumping. Only a real seek, a new track or a pause lands at once.

/** How often the store is refreshed. Four times a second turns a lyric line
 *  over within a quarter of a second of its cue, at no cost worth counting. */
const TICK_MS = 250;
/** Behind our clock by more than this is the listener seeking back. The
 *  server already holds a stale report, so anything it says this far back is
 *  meant. */
const SNAP_BACK_MS = 3_000;
/** Ahead by up to this is glided into; further is a seek forward. The first
 *  fresh report after a track starts can be several seconds ahead, because the
 *  first number a polled source hands over may already be old. */
const GLIDE_AHEAD_MS = 8_000;
/** A difference is absorbed over at least this long, and over its own length
 *  when it is bigger — so the clock never runs faster than double speed, and
 *  never stops or runs backwards while slowing down. */
const GLIDE_MIN_MS = 4_000;
/** Close enough that correcting would be noise. */
const AGREE_MS = 150;

let anchorMs = 0;
let anchorAt = 0;
let correctionMs = 0;
let glideMs = GLIDE_MIN_MS;
let held = false;
let durationMs = Infinity;

/** Where the track is by the clock, in milliseconds. */
function clockMs(at = performance.now()): number {
	if (held) return Math.min(durationMs, anchorMs);
	const since = Math.max(0, at - anchorAt);
	const absorbed = correctionMs * Math.min(1, since / glideMs);
	return Math.min(durationMs, Math.max(0, anchorMs + since + absorbed));
}

function anchor(ms: number, at: number, correction = 0) {
	anchorMs = ms;
	anchorAt = at;
	correctionMs = correction;
	glideMs = Math.max(GLIDE_MIN_MS, Math.abs(correction));
}

// The browser half of the `playhead` trace: one console line per poll saying
// what the server reported, where this tab's clock was, and what it did about
// the difference. Off unless asked for, from the console:
//   localStorage.setItem('td-debug-playhead', '1')
function tracing(): boolean {
	try {
		return localStorage.getItem('td-debug-playhead') === '1';
	} catch {
		return false;
	}
}
let lastPollAt = 0;

let watchers = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let tickTimer: ReturnType<typeof setInterval> | null = null;

async function poll() {
	try {
		const s = await api.getNowPlaying();
		const at = performance.now();
		if (!s.playing || !s.track) {
			deck.set(EMPTY);
			return;
		}
		const track = s.track;
		const serverMs = s.position_ms ?? (s.position ?? 0) * 1000;
		deck.update((d) => {
			const sameTrack =
				d.playing && d.track?.title === track.title && d.track?.artist === track.artist;
			const wasHeld = held;
			const local = clockMs(at);
			const diff = serverMs - local;
			held = !!track.paused;
			durationMs = track.duration ? track.duration * 1000 : Infinity;

			let verdict: string;
			if (!sameTrack) {
				anchor(serverMs, at);
				verdict = 'new track';
			} else if (held || wasHeld) {
				// Pausing or resuming: the server's number is where it stopped.
				anchor(serverMs, at);
				verdict = held ? 'paused' : 'resumed';
			} else if (diff < -SNAP_BACK_MS || diff > GLIDE_AHEAD_MS) {
				anchor(serverMs, at);
				verdict = `JUMPED ${diff > 0 ? 'forward' : 'BACK'} ${(Math.abs(diff) / 1000).toFixed(1)}s (seek)`;
			} else if (Math.abs(diff) <= AGREE_MS) {
				anchor(local, at);
				verdict = 'agrees';
			} else {
				anchor(local, at, diff);
				verdict = `gliding ${diff > 0 ? '+' : ''}${(diff / 1000).toFixed(2)}s over ${(glideMs / 1000).toFixed(1)}s`;
			}

			if (tracing()) {
				console.debug(
					`[playhead] '${track.title}' server=${serverMs}ms ours=${Math.round(local)}ms (${verdict}) ` +
						`Δpoll=${lastPollAt ? Math.round(at - lastPollAt) : '-'}ms known=${track.position_known}`
				);
				lastPollAt = at;
			}
			return { playing: true, track, position: clockMs(at) / 1000 };
		});
	} catch {
		// A failed poll (offline, session expired) just leaves the deck as-is
		// rather than flickering it empty; the next poll corrects it.
	}
}

function tick() {
	deck.update((d) => {
		if (!d.playing || !d.track) return d;
		// Held at the end by `clockMs` — the next poll decides whether it's
		// still playing.
		return { ...d, position: clockMs() / 1000 };
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
		tickTimer = setInterval(tick, TICK_MS);
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
