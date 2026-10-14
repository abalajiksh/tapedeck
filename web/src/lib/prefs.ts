import { writable } from 'svelte/store';
import { browser } from '$app/environment';
import { api } from './api';
import { adoptLyricView } from './lyricview';

/**
 * Display preferences that several screens read. Server-persisted like the
 * theme, with localStorage as the no-flash cache so a list doesn't render
 * letter tiles and then swap to artwork a moment later.
 */

const ARTWORK_KEY = 'td-artwork';

function cached(): boolean {
	if (!browser) return true;
	const v = localStorage.getItem(ARTWORK_KEY);
	return v === null ? true : v === '1';
}

export const showArtwork = writable<boolean>(cached());

/** Pull the stored value once per session; the layout calls this after auth. */
export async function initPrefs() {
	if (!browser) return;
	try {
		const prefs = await api.getPreferences();
		adoptLyricView(prefs.lyrics_view);
		if (typeof prefs.show_artwork === 'boolean') {
			showArtwork.set(prefs.show_artwork);
			localStorage.setItem(ARTWORK_KEY, prefs.show_artwork ? '1' : '0');
		}
	} catch {
		// Keep the cached value — a failed read shouldn't flip the setting.
	}
}

export function setShowArtwork(on: boolean) {
	showArtwork.set(on);
	if (!browser) return;
	localStorage.setItem(ARTWORK_KEY, on ? '1' : '0');
	api.putPreferences({ show_artwork: on }).catch(() => {});
}
