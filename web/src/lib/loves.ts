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

/**
 * The forgiving fold, mirroring `Database::match_key` in `db.rs`.
 *
 * Two players rarely spell one song identically: Plex files a listen under the
 * record's artist while a scrobble client reports what the player displayed, so
 * anything with a feature arrives under two names — and an apostrophe, an
 * accent or a ligature does the same. Without this the heart filled only when
 * the two happened to agree.
 *
 * **This must stay character-for-character in step with the Rust side.** The
 * server pins the same cases in `the_folded_key_is_exactly_what_the_client_must_reproduce`,
 * listed there in this order:
 *
 *   Burial / Archangel                 → recording|burial|archangel
 *   Burial feat. Someone / Archangel   → recording|burial|archangel
 *   Cœur de pirate / Don’t Look Back   → recording|coeurdepirate|dontlookback
 *   Beyoncé / Halo                     → recording|beyonce|halo
 *   A.R. Rahman / Roja                 → release|arrahman|roja
 *   Simon & Garfunkel (artist)         → artist||simongarfunkel
 *   Sigur Rós / Hoppípolla             → recording|sigurros|hoppipolla
 *
 * If they drift, hearts silently stop filling — nothing errors.
 */
const LIGATURES: Record<string, string> = {
	œ: 'oe', Œ: 'oe', æ: 'ae', Æ: 'ae', ß: 'ss',
	ø: 'o', Ø: 'o', đ: 'd', Đ: 'd', ð: 'd', Ð: 'd',
	ł: 'l', Ł: 'l', þ: 'th', Þ: 'th'
};

/** `normalise_artist` in `db.rs`: expand the ligatures Unicode won't decompose,
 *  then NFKD, drop combining marks, keep only alphanumerics, lowercase. */
function fold(s: string): string {
	return [...s]
		.map((c) => LIGATURES[c] ?? c)
		.join('')
		.normalize('NFKD')
		.replace(/\p{M}/gu, '')
		.toLowerCase()
		.replace(/[^\p{L}\p{N}]/gu, '');
}

/** `split_featured` in `db.rs`: the primary of an explicit credit. Only these
 *  forms — `&` and `,` are names ("Simon & Garfunkel"), not credits. */
const CREDITS = [' feat. ', ' feat ', ' ft. ', ' ft ', ' featuring ', ' with '];
function primaryArtist(artist: string): string {
	const lower = artist.toLowerCase();
	for (const m of CREDITS) {
		const at = lower.indexOf(m);
		if (at > 0) return artist.slice(0, at);
	}
	return artist;
}

function foldedKey(kind: string, artist: string, name: string): string {
	return `${kind}|${fold(primaryArtist(artist))}|${fold(name)}`;
}

/** The entity key for one aspect of a listen, or null if the row can't name it
 *  (a listen with no album can't have an album key).
 *
 *  This is the *exact* key, kept because it is what an optimistic toggle adds
 *  to the set. Matching a heart goes through `isLoved`, which also tries the
 *  folded and MusicBrainz forms. */
export function loveKey(s: Scrobble, kind: LoveKind): string | null {
	if (kind === 'artist') return `artist||${norm(s.artist)}`;
	if (kind === 'album') {
		if (!s.album) return null;
		return `release|${norm(s.artist)}|${norm(s.album)}`;
	}
	return `recording|${norm(s.artist)}|${norm(s.title)}`;
}

/**
 * Every key a listen row could be recognised by — exact, folded, and its
 * MusicBrainz id.
 *
 * The id is the strongest of the three and the one that was missing entirely:
 * a Plex listen and a scrobble-client listen of the same track carried the
 * identical recording id and still counted as two different things.
 */
export function loveCandidates(s: Scrobble, kind: LoveKind): string[] {
	const out: string[] = [];
	const exact = loveKey(s, kind);
	if (!exact) return out;
	out.push(exact);

	if (kind === 'artist') out.push(foldedKey('artist', '', s.artist));
	else if (kind === 'album') out.push(foldedKey('release', s.artist, s.album!));
	else out.push(foldedKey('recording', s.artist, s.title));

	// Only the recording id travels on a listen row — an album or artist id
	// would need columns the row does not carry.
	if (kind === 'track' && s.mbid_recording) {
		out.push(`mbid:recording:${s.mbid_recording.trim().toLowerCase()}`);
	}
	return out;
}

/** Is any way of naming this row's track/album/artist loved? */
export function isLoved(set: Set<string>, s: Scrobble, kind: LoveKind): boolean {
	return loveCandidates(s, kind).some((k) => set.has(k));
}

/** The same question for something named directly rather than via a listen. */
export function isLovedNamed(
	set: Set<string>,
	kind: LoveKind,
	artist: string,
	name: string
): boolean {
	const exact = loveKeyOf(kind, artist, name);
	if (!exact) return false;
	const folded =
		kind === 'artist'
			? foldedKey('artist', '', artist)
			: foldedKey(kind === 'album' ? 'release' : 'recording', artist, name);
	return set.has(exact) || set.has(folded);
}

/** The folded key for something named directly. */
function foldedKeyFor(kind: LoveKind, artist: string, name: string): string {
	return kind === 'artist'
		? foldedKey('artist', '', artist)
		: foldedKey(kind === 'album' ? 'release' : 'recording', artist, name);
}

/**
 * Apply an optimistic toggle across every form the thing can be recognised by.
 *
 * Un-loving has to remove **all** of them, not just the exact key: the heart may
 * well have been filled by the folded or MusicBrainz form — that is the whole
 * point — and dropping only the exact one leaves it stubbornly filled until the
 * next reload. Loving adds them all for the mirror-image reason.
 */
function applyLove(before: Set<string>, wanted: boolean, keys: string[]): Set<string> {
	const next = new Set(before);
	for (const k of keys) {
		if (wanted) next.add(k);
		else next.delete(k);
	}
	return next;
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
	const wanted = !isLovedNamed(before, kind, artist, name);
	lovedKeys.set(applyLove(before, wanted, [key, foldedKeyFor(kind, artist, name)]));

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
	const wanted = !isLoved(before, s, kind);
	lovedKeys.set(applyLove(before, wanted, loveCandidates(s, kind)));

	try {
		await api.setLove({ from_scrobble: s.id, kind }, wanted);
	} catch {
		lovedKeys.set(before);
	}
}
