import { writable, get } from 'svelte/store';
import { api, type Scrobble } from './api';

/**
 * Which things the user loves, as a set of entity keys.
 *
 * Loves are stored against entities, but entities are only created once
 * something is annotated or loved — so a history row usually has no entity id
 * to ask about, and asking per row would be one request per listen. The key is
 * deterministic (`kind|artist|name`, lowercased and trimmed — `entity_key` in
 * `db.rs`), so the whole set is fetched once and matched client-side.
 *
 * Keep `loveKey` in step with `Database::entity_key`. If the two ever disagree,
 * hearts silently stop filling in — nothing errors.
 */
export const lovedKeys = writable<Set<string>>(new Set());

let loaded = false;

export type LoveKind = 'track' | 'album' | 'artist';

/** The stored `entities.kind` each UI kind maps to. */
const ENTITY_KIND: Record<LoveKind, string> = {
	track: 'recording',
	album: 'release',
	artist: 'artist'
};

function norm(s: string): string {
	return s.trim().toLowerCase();
}

/** The entity key for one aspect of a listen, or null if the row can't name it
 *  (a listen with no album can't have an album key). */
export function loveKey(s: Scrobble, kind: LoveKind): string | null {
	if (kind === 'artist') return `artist||${norm(s.artist)}`;
	if (kind === 'album') {
		if (!s.album) return null;
		return `release|${norm(s.artist)}|${norm(s.album)}`;
	}
	return `recording|${norm(s.artist)}|${norm(s.title)}`;
}

/** Fetch the set once per page load. Safe to call from several components. */
export async function loadLoves(force = false): Promise<void> {
	if (loaded && !force) return;
	loaded = true;
	try {
		const { keys } = await api.getLoveKeys();
		lovedKeys.set(new Set(keys));
	} catch {
		// Loves are decoration on a listen row — a failure here must not stop
		// the history from rendering.
		loaded = false;
	}
}

/** The entity key for a track named directly, rather than via a listen row. */
export function loveKeyOf(kind: LoveKind, artist: string, name: string): string | null {
	if (kind === 'artist') return `artist||${norm(artist)}`;
	if (!name.trim()) return null;
	return `${kind === 'album' ? 'release' : 'recording'}|${norm(artist)}|${norm(name)}`;
}

/**
 * Toggle a love for something identified by name.
 *
 * The deck is the case this exists for: now-playing is in-memory and never
 * persisted, so the track being played has no scrobble id to hang a love off —
 * and it may not have been scrobbled yet at all, since that happens at the
 * halfway mark. The API's target spec already accepts `{kind, name, artist}`
 * and resolves (or creates) the entity from those, so no listen is needed.
 *
 * Optimistic in the same way as `toggleLove`, and shares `lovedKeys`, so a
 * heart filled on the deck is filled on the history row for the same track.
 */
export async function toggleLoveByName(
	kind: LoveKind,
	artist: string,
	name: string
): Promise<void> {
	const key = loveKeyOf(kind, artist, name);
	if (!key) return;

	const before = get(lovedKeys);
	const next = new Set(before);
	const wanted = !before.has(key);
	if (wanted) next.add(key);
	else next.delete(key);
	lovedKeys.set(next);

	try {
		await api.setLove({ kind, name: kind === 'artist' ? artist : name, artist }, wanted);
	} catch {
		lovedKeys.set(before);
	}
}

/**
 * Toggle a love from a listen row.
 *
 * Optimistic: the key is flipped locally first so the heart responds to the
 * click, and put back if the server disagrees. The server resolves (and
 * creates) the entity from the listen, which is why this takes a scrobble id
 * rather than an entity id.
 */
export async function toggleLove(s: Scrobble, kind: LoveKind): Promise<void> {
	const key = loveKey(s, kind);
	if (!key) return;

	const before = get(lovedKeys);
	const next = new Set(before);
	const wanted = !before.has(key);
	if (wanted) next.add(key);
	else next.delete(key);
	lovedKeys.set(next);

	try {
		await api.setLove({ from_scrobble: s.id, kind }, wanted);
	} catch {
		lovedKeys.set(before);
	}
}
