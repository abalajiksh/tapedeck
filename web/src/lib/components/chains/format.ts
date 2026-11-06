// Small wording helpers shared by the Signal Chains list, drawer and graph.

import type { ChainVariant, SignalChain } from '$lib/api';

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

/** Positions in a chain that a named setup swaps, so a row can mark them. */
export function swappedPositions(c: SignalChain, variants: ChainVariant[]): Set<number> {
	const at = new Set<number>();
	for (const v of variants)
		if (v.chain_id === c.id) for (const o of v.overrides ?? []) at.add(o.position);
	return at;
}
