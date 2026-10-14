/** What a romanized lyric is shown as: its own script, Latin letters, or both.
 *
 * One choice for the whole app — the reader, the deck strip and the notes
 * panel read the same store — remembered per account (`lyrics_view`) like the
 * other display preferences, with localStorage as the no-flash cache.
 * Original is the default: romanization is offered, never imposed.
 */
import { writable } from 'svelte/store';
import { browser } from '$app/environment';
import { api } from './api';

export type LyricView = 'original' | 'romanized' | 'both';

const KEY = 'td-lyric-view';

function cached(): LyricView {
	try {
		const v = browser ? localStorage.getItem(KEY) : null;
		if (v === 'original' || v === 'romanized' || v === 'both') return v;
	} catch {
		/* private window, or site data blocked — the default is fine */
	}
	return 'original';
}

export const lyricView = writable<LyricView>(cached());

/** Adopt the stored choice; `initPrefs` calls this with the preferences it read. */
export function adoptLyricView(v: unknown) {
	if (v !== 'original' && v !== 'romanized' && v !== 'both') return;
	lyricView.set(v);
	try {
		localStorage.setItem(KEY, v);
	} catch {
		/* not worth failing over */
	}
}

export function setLyricView(v: LyricView) {
	adoptLyricView(v);
	api.putPreferences({ lyrics_view: v }).catch(() => {});
}

export const VIEW_LABEL: Record<LyricView, string> = {
	original: 'Original',
	romanized: 'Romanized',
	both: 'Both'
};
