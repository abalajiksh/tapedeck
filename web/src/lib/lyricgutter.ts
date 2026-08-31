/** What the gutter beside a lyric line shows.
 *
 * The design's three-way cycle: the cue time, the line number, or nothing.
 * They answer different questions — a timestamp is for finding a place in the
 * music, a number is for writing `[[line:12]]`, and neither is wanted when you
 * are only reading — so this is a control rather than something inferred.
 *
 * It *was* inferred, which is the bug this fixes: both panels drew the cue when
 * the lyric was timed and the number when it was not, so on a timed lyric the
 * number a reference is written from was never on screen at all.
 *
 * A fact about the browser in front of you rather than about the account, so
 * localStorage rather than a preference on the server — the same call the
 * annotate panel's own open/closed state makes.
 */
import { writable } from 'svelte/store';

export type Gutter = 'time' | 'number' | 'none';

const KEY = 'td-lyric-gutter';
const ORDER: Gutter[] = ['time', 'number', 'none'];

function initial(): Gutter {
	try {
		const v = localStorage.getItem(KEY);
		if (v === 'time' || v === 'number' || v === 'none') return v;
	} catch {
		/* private window, or site data blocked — the default is fine */
	}
	return 'time';
}

export const gutter = writable<Gutter>(initial());

gutter.subscribe((v) => {
	try {
		localStorage.setItem(KEY, v);
	} catch {
		/* not worth failing a render over */
	}
});

/** Step to the next mode. Wraps, so the control never dead-ends. */
export function cycleGutter(): void {
	gutter.update((v) => ORDER[(ORDER.indexOf(v) + 1) % ORDER.length]);
}

/** What the gutter should read for a line, or `''` when it shows nothing.
 *
 * A timed lyric asked for cues still falls back to the number on a line that
 * has none — a synced file can legitimately leave one uncued, and an empty
 * gutter cell there would look like a rendering fault rather than a gap in the
 * lyric.
 */
export function gutterLabel(
	mode: Gutter,
	n: number,
	at: number | null | undefined,
	mmss: (s: number) => string
): string {
	if (mode === 'none') return '';
	if (mode === 'number') return String(n);
	return at != null ? mmss(at) : String(n);
}

/** The label for the control itself, so both panels cannot word it differently. */
export function gutterTitle(mode: Gutter): string {
	return mode === 'time'
		? 'Gutter: cue times — click for line numbers'
		: mode === 'number'
			? 'Gutter: line numbers — click to hide'
			: 'Gutter: hidden — click for cue times';
}
