import { goto } from '$app/navigation';
import { api } from './api';

/**
 * Navigating to an album or artist page from a listen row.
 *
 * The row knows names, not entity ids — and the entity may not exist at all yet,
 * since entities are only created when something is annotated, loved, or (now)
 * visited. So this is a round-trip: resolve the name to an id, then go there.
 * That's why these are click handlers rather than plain `<a href>`s.
 *
 * A failed resolve falls back to the history filtered by the same name, which is
 * a worse page but never a dead click.
 */
async function open(
	kind: 'album' | 'artist',
	name: string,
	artist: string | undefined,
	fallback: string
): Promise<void> {
	try {
		const { id } = await api.resolveEntity(kind, name, artist);
		await goto(`/${kind}/${id}`);
	} catch {
		await goto(fallback);
	}
}

export function openAlbum(artist: string, album: string): Promise<void> {
	return open('album', album, artist, `/history?album=${encodeURIComponent(album)}`);
}

export function openArtist(artist: string): Promise<void> {
	return open('artist', artist, undefined, `/history?artist=${encodeURIComponent(artist)}`);
}
