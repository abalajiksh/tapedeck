/**
 * Everyone credited on a track, primary first.
 *
 * `credit` is the server's `artist_credit`: every name joined with `"; "`, the
 * primary first, present only when there is more than one. A single entry is
 * not a list — it's what the old `feat.` fold left there — so the artist stands
 * alone. When the artist is one of the names (a primary the source chose, as
 * Plex and Jellyfin do) it leads; otherwise it is a display string such as
 * "Lady Gaga, Bruno Mars" and the list replaces it.
 */
export function creditedNames(artist: string, credit: string | null | undefined): string[] {
	const names = (credit ?? '').split(';').map((n) => n.trim()).filter(Boolean);
	if (names.length < 2) return artist ? [artist] : [];
	const lower = artist.trim().toLowerCase();
	const i = names.findIndex((n) => n.toLowerCase() === lower);
	if (i < 0) return names;
	return [names[i], ...names.filter((_, j) => j !== i)];
}
