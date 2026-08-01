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
