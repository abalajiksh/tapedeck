import { writable } from 'svelte/store';
import { browser } from '$app/environment';

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
	// hydrate from the server (source of truth across devices)
	fetch('/api/v1/preferences', { credentials: 'same-origin' })
		.then((r) => (r.ok ? r.json() : null))
		.then((p) => {
			if (p?.theme) setTheme(p.theme, false);
		})
		.catch(() => {});
}

export function setTheme(mode: ThemeMode, persist = true) {
	theme.set(mode);
	apply(mode);
	if (!browser) return;
	localStorage.setItem('td-theme', mode);
	if (persist) {
		fetch('/api/v1/preferences', {
			method: 'PUT',
			credentials: 'same-origin',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ theme: mode })
		}).catch(() => {}); // localStorage remains the offline fallback
	}
}
