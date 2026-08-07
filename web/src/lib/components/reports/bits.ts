/**
 * Shared formatting for the three chapter layouts.
 *
 * Kept in one place so a change to how a delta reads lands on the week, month
 * and year cards together — they show the same numbers at different scales,
 * and the fastest way to make them disagree is to format them separately.
 */
export const nf = (n: number) => n.toLocaleString();

export function delta(now: number, before: number | undefined | null): number | null {
	if (before == null || before === 0) return null;
	return Math.round(((now - before) / before) * 100);
}

export function deltaText(d: number | null): string {
	if (d == null) return '';
	return `${d > 0 ? '▲' : d < 0 ? '▼' : '—'} ${Math.abs(d)}%`;
}

/** "3rd", "1st" — because "rank 3" reads as a score rather than a placing. */
export function ordinal(n: number): string {
	const s = ['th', 'st', 'nd', 'rd'];
	const v = n % 100;
	return n + (s[(v - 20) % 10] ?? s[v] ?? s[0]);
}
