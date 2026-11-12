// Cases for tapedeck.yml's TypeScript rules: `opengrep test .opengrep`.

export function urlState(url: string) {
	// ruleid: history-replacestate
	history.replaceState(null, '', url);
	// ruleid: history-replacestate
	window.history.replaceState({}, '', url);
	// ok: history-replacestate
	syncUrl(url);
}

export async function calls() {
	// ruleid: fetch-outside-api-ts
	const r = await fetch('/api/v1/preferences', { credentials: 'same-origin' });
	// ok: fetch-outside-api-ts
	const p = await api.getPreferences();
}
