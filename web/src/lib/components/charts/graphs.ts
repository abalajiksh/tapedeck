/**
 * Shared pieces for the Statistics graphs (`/api/v1/stats/graphs`).
 *
 * Days arrive as an axis — a `start_day` and one count per day — already
 * bucketed in the listener's own timezone on the server. Everything here works
 * on those calendar dates in **UTC** on purpose: the date is the fact, and
 * reading it through the browser's zone would shift a day across a DST change
 * or for anyone viewing from elsewhere.
 */
import { scaleUtc } from 'd3-scale';

/** Categorical series colours, in their fixed order. Never cycled: a seventh
 *  series is a bug in the caller, not a reason to invent a hue. */
export const SERIES = [1, 2, 3, 4, 5, 6].map((i) => `var(--td-series-${i})`);

const DAY_MS = 86_400_000;

/** `YYYY-MM-DD` → a Date at UTC midnight. */
export function parseDay(day: string): Date {
	const [y, m, d] = day.split('-').map(Number);
	return new Date(Date.UTC(y, (m ?? 1) - 1, d ?? 1));
}

/** The date `i` days after `start`. */
export function dayAt(start: string, i: number): Date {
	return new Date(parseDay(start).getTime() + i * DAY_MS);
}

/** A calendar date in words, read as the date it is (UTC, see above). */
export function fmtDate(d: Date, withWeekday = false): string {
	return d.toLocaleDateString(undefined, {
		timeZone: 'UTC',
		day: 'numeric',
		month: 'short',
		year: 'numeric',
		...(withWeekday ? { weekday: 'short' } : {})
	});
}

/** Unix seconds as a date — for first/last listens, which are instants. */
export function fmtTs(ts: number): string {
	return new Date(ts * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

export const nf = (n: number) => Math.round(n).toLocaleString();
export const pct = (x: number, digits = 0) => `${(x * 100).toFixed(digits)}%`;

/** Trailing mean over `window` days. The first days average what exists so
 *  far rather than padding with zeros, which would invent a ramp-up. */
export function rolling(values: number[], window: number): number[] {
	if (window <= 1) return values.slice();
	const out: number[] = new Array(values.length);
	let sum = 0;
	for (let i = 0; i < values.length; i++) {
		sum += values[i];
		if (i >= window) sum -= values[i - window];
		out[i] = sum / Math.min(i + 1, window);
	}
	return out;
}

/**
 * Tick positions for a day axis of `n` days starting at `start`: about
 * `count` of them, at nice dates (week starts, month starts, years), each
 * labelled at the granularity the span calls for.
 */
export function dayTicks(start: string, n: number, count = 6): { i: number; label: string }[] {
	if (n <= 0) return [];
	const d0 = parseDay(start);
	const d1 = new Date(d0.getTime() + (n - 1) * DAY_MS);
	const ticks = scaleUtc().domain([d0, d1]).ticks(count);
	const spanDays = n;
	const opts: Intl.DateTimeFormatOptions =
		spanDays > 3 * 365
			? { year: 'numeric' }
			: spanDays > 75
				? { month: 'short', year: '2-digit' }
				: { day: 'numeric', month: 'short' };
	return ticks.map((t) => ({
		i: Math.round((t.getTime() - d0.getTime()) / DAY_MS),
		label: t.toLocaleDateString(undefined, { timeZone: 'UTC', ...opts })
	}));
}

/** Pointer x inside an SVG, in the plot's own coordinates. */
export function pointerX(e: PointerEvent, left: number): number {
	const svg = (e.currentTarget as SVGElement).ownerSVGElement ?? (e.currentTarget as SVGSVGElement);
	return e.clientX - svg.getBoundingClientRect().left - left;
}

/** Nice round gridlines for a 0-based value axis. */
export function valueTicks(max: number, count = 4): number[] {
	if (max <= 0) return [0];
	const raw = max / count;
	const mag = 10 ** Math.floor(Math.log10(raw));
	const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
	const out: number[] = [];
	for (let v = 0; v <= max + 1e-9; v += step) out.push(Math.round(v * 100) / 100);
	return out;
}
