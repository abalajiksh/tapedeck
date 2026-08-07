/**
 * How to present a fidelity share.
 *
 * The server computes lossless and DSD over the listens whose format it
 * actually knows (`FIDELITY_KNOWN` in `db.rs`), not over everything — a listen
 * imported from Last.fm carries no format, and on a real 21,390-listen history
 * 97.4% of rows carry none at all. That fixes the number. It does not fix the
 * *reading*: "94%" derived from 558 of 21,390 listens invites being read as a
 * fact about the whole history, when it is a fact about the 558.
 *
 * So below a coverage floor the card stops scoring and reports its coverage
 * instead. Same discipline as the genre trajectory drawing a period faintly
 * below 90% enrichment, and as the neighbours page saying "run the deep scrape"
 * rather than showing bare cards that look like a bug.
 *
 * The floor is a *presentation* threshold, not a measurement parameter — it
 * changes what is said, never what is computed — which is why it can be a round
 * number without the objection that sank the grid-based breadth metric.
 */
export const FIDELITY_FLOOR = 0.25;

export interface Fidelity {
	/** True when there is enough known-format listening to put a number up. */
	scored: boolean;
	/** `"94%"`, or `"3%"` of coverage when not scored. */
	value: string;
	/** The line under the number. Always says what the number is *of*. */
	sub: string;
}

/**
 * @param pct    the share the server computed, already over known-format plays
 * @param known  how many listens that was
 * @param total  every play in the same window
 * @param label  what is being shared — "lossless", "DSD"
 */
export function fidelity(pct: number, known: number, total: number, label: string): Fidelity {
	if (!total || !known) {
		return { scored: false, value: '—', sub: 'no formats recorded yet' };
	}
	const coverage = known / total;
	if (coverage < FIDELITY_FLOOR) {
		return {
			scored: false,
			value: `${Math.max(1, Math.round(coverage * 100))}%`,
			sub: 'of your listening has a known format — too little to score'
		};
	}
	return {
		scored: true,
		value: `${pct}%`,
		sub: `of ${known.toLocaleString()} listens with a known format`
	};
}

/** The heading the card wears. It stops claiming to measure fidelity when it
 *  isn't measuring any. */
export function fidelityLabel(f: Fidelity, scoredLabel: string): string {
	return f.scored ? scoredLabel : 'Fidelity';
}
