// Small wording helpers shared by the Signal Chains list, drawer and graph.

import type { ChainVariant, Equipment, SignalChain } from '$lib/api';

/** "Today", a weekday within the week, otherwise "29 Oct" (with the year
 *  when it is not this one). The list is scanned, not read, so this is short. */
export function lastUsedLabel(ts: number | null | undefined): string {
	if (!ts) return 'Never used yet';
	const d = new Date(ts * 1000);
	const now = new Date();
	const days = Math.floor((now.getTime() - d.getTime()) / 86_400_000);
	if (d.toDateString() === now.toDateString()) return 'Today';
	if (days < 7) return d.toLocaleDateString(undefined, { weekday: 'long' });
	return d.toLocaleDateString(undefined, {
		day: 'numeric',
		month: 'short',
		...(d.getFullYear() === now.getFullYear() ? {} : { year: 'numeric' })
	});
}

/** The same, as the end of a sentence: "last used Tuesday", "never used yet".
 *  Only the two fixed phrases change case; a weekday or month keeps its own. */
export function lastUsedPhrase(ts: number | null | undefined): string {
	if (!ts) return 'never used yet';
	const label = lastUsedLabel(ts);
	return `last used ${label === 'Today' ? 'today' : label}`;
}

/** Positions in a chain that are slots (two or more options), so a row can
 *  mark them. */
export function slotPositions(c: SignalChain): Set<number> {
	const at = new Set<number>();
	(c.components ?? []).forEach((s, i) => {
		if ((s.options?.length ?? 0) >= 2) at.add(i);
	});
	return at;
}

/** What a variant puts on, by gear name: "Hexa", "Shibata + HD 650". */
export function picksLabel(v: ChainVariant, equipment: Equipment[]): string {
	return (v.overrides ?? [])
		.map((o) => o.name ?? equipment.find((e) => e.id === o.equipment_id)?.name ?? 'other gear')
		.join(' + ');
}

/** How a variant reads on screen. Tapedeck's own (`auto`) are named by what
 *  they put on, never by their internal name; a setup someone named before
 *  slots existed keeps its name. */
export function variantLabel(v: ChainVariant, equipment: Equipment[]): string {
	// The old route a "from now on" route edit keeps for past plays is named
	// for when it ended, which says more than its list of differences.
	if (v.auto && v.name.startsWith('Route until')) return v.name;
	return v.auto ? picksLabel(v, equipment) || 'as wired' : v.name;
}
