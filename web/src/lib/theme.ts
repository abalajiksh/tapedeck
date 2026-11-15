import { writable } from 'svelte/store';
import { browser } from '$app/environment';
import { api } from '$lib/api';

export type ThemeMode = 'light' | 'dark' | 'system';

function systemDark() {
	return browser && matchMedia('(prefers-color-scheme: dark)').matches;
}
function apply(mode: ThemeMode) {
	if (!browser) return;
	const dark = mode === 'dark' || (mode === 'system' && systemDark());
	document.documentElement.dataset.theme = dark ? 'dark' : 'light';
}

export const theme = writable<ThemeMode>('system');

export function initTheme() {
	if (!browser) return;
	const stored = (localStorage.getItem('td-theme') as ThemeMode) || 'system';
	theme.set(stored);
	apply(stored);
	// keep "system" live
	matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () =>
		theme.update((m) => (apply(m), m))
	);
	// hydrate from the server (source of truth across devices). Through
	// api.ts, which throws on a failed response: signed out (the login page),
	// that is a 401, and localStorage stays the answer.
	api
		.getPreferences()
		.then((p) => {
			if (typeof p.theme === 'string') setTheme(p.theme as ThemeMode, false);
		})
		.catch(() => {});
}

export function setTheme(mode: ThemeMode, persist = true) {
	theme.set(mode);
	apply(mode);
	if (!browser) return;
	localStorage.setItem('td-theme', mode);
	if (persist) {
		api.putPreferences({ theme: mode }).catch(() => {}); // localStorage remains the offline fallback
	}
}
