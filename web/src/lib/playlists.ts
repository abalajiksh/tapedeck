import { api, type PlaylistTarget } from './api';

/**
 * Where a generated list can be sent.
 *
 * Fetched once per page load and shared: Rediscovery alone renders seven cards
 * that each want a push button, and asking the server seven times for the same
 * unchanging answer is the sort of thing that only shows up on someone else's
 * hardware.
 */
let pending: Promise<PlaylistTarget[]> | null = null;

export function playlistTargets(): Promise<PlaylistTarget[]> {
	pending ??= api
		.playlistTargets()
		.then((r) => r.targets)
		.catch(() => {
			// A failure must not be cached — otherwise a server that was briefly
			// unreachable hides the button for the rest of the session. Empty is
			// the right answer for now: with no targets there is nothing to
			// offer, and a banner on every card would be worse than a quiet
			// absence.
			pending = null;
			return [];
		});
	return pending;
}

/** Forget the cached answer — call after the user adds or removes a source. */
export function forgetPlaylistTargets(): void {
	pending = null;
}

/**
 * A default name for a pushed playlist.
 *
 * Prefixed, because these land in a library beside playlists the user made by
 * hand and they should be able to tell at a glance which ones this app put
 * there. Dated, because pushing "Forgotten Favourites" twice in a year should
 * give two playlists rather than a mystery about which is current.
 */
export function suggestPlaylistName(title: string): string {
	const when = new Date().toLocaleDateString(undefined, { month: 'short', year: 'numeric' });
	return `Tapedeck · ${title} · ${when}`;
}
