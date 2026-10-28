<script lang="ts">
	/**
	 * Your history with one artist: when they took off, habit or burst, the
	 * days, months and hours, runs, playthroughs and weeks at number one.
	 *
	 * Every proportion is drawn from a pair the server returned together — their
	 * listens and everyone's over the same skip-free window — so nothing here
	 * divides two numbers that were counted differently.
	 */
	import type { ArtistDossier, StatCard } from '$lib/api';
	import { formatDateTime } from '$lib/utils';
	import StatStrip from './StatStrip.svelte';

	export let d: ArtistDossier;
	export let name: string;

	const DAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
	const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
	const DAY_MS = 86_400_000;

	/** Whole percent, but never a "0%" for something that happened. */
	function pct(part: number, whole: number): string {
		if (!whole) return '—';
		const p = (100 * part) / whole;
		return p > 0 && p < 1 ? '<1%' : `${Math.round(p)}%`;
	}
	/** `YYYY-MM-DD` as written, with no timezone to shift it. */
	function iso(date: string): { y: number; m: number; day: number } {
		const [y, m, day] = date.split('-').map(Number);
		return { y, m, day };
	}
	function longDate(date: string): string {
		const { y, m, day } = iso(date);
		return `${day} ${MONTHS[m - 1]} ${y}`;
	}
	function plural(n: number, one: string, many = `${one}s`): string {
		return `${n.toLocaleString()} ${n === 1 ? one : many}`;
	}

	// ── The strip ──
	$: cards = ((): StatCard[] => {
		const out: StatCard[] = [
			{
				k: 'Days',
				v: d.days.toLocaleString(),
				sub: `of your ${d.all_days.toLocaleString()} listening days (${pct(d.days, d.all_days)})`,
				tone: 'plain'
			}
		];
		if (d.runs.length) {
			out.push({
				k: 'Longest run',
				v: `${d.runs[0].length} in a row`,
				sub: formatDateTime(d.runs[0].start),
				tone: 'plain'
			});
		}
		// Only when it happened: for nearly every artist it is a zero, and a
		// card of zeros reads as something missing.
		if (d.months_at_top.periods || d.weeks_at_top.periods) {
			out.push({
				k: 'Months at #1',
				v: d.months_at_top.periods.toLocaleString(),
				sub: `of ${d.months_at_top.of} · ${plural(d.weeks_at_top.periods, 'week')} at #1`,
				tone: 'plain'
			});
		}
		if (d.took_off) {
			const { y, m } = iso(d.took_off);
			out.push({
				k: 'Took off',
				v: `${MONTHS[m - 1]} ${y}`,
				sub: `fastest two-week climb, ${longDate(d.took_off)}`,
				tone: 'dark'
			});
		}
		return out;
	})();

	// ── Habit or burst ──
	$: verdict = !d.habit
		? ''
		: d.habit.score >= 60
			? 'A habit'
			: d.habit.score >= 35
				? 'Comes and goes'
				: 'In bursts';

	// ── Calendar: one column per week, from their first day to their last ──
	$: playthroughDays = new Set(d.playthroughs.map((p) => p.date));
	$: byDate = new Map(d.daily.map((x) => [x.date, x]));
	$: maxDay = Math.max(1, ...d.daily.map((x) => x.plays));
	$: calendar = ((): { date: string; plays: number; all: number; col: number; row: number }[] => {
		if (d.daily.length < 2) return [];
		const a = iso(d.daily[0].date);
		const b = iso(d.daily[d.daily.length - 1].date);
		const first = Date.UTC(a.y, a.m - 1, a.day);
		const last = Date.UTC(b.y, b.m - 1, b.day);
		// Back to the start of that week, so rows are weekdays in your order.
		const lead = (new Date(first).getUTCDay() - d.week_start + 7) % 7;
		const out = [];
		for (let t = first - lead * DAY_MS, i = 0; t <= last; t += DAY_MS, i++) {
			const date = new Date(t).toISOString().slice(0, 10);
			const hit = byDate.get(date);
			out.push({ date, plays: hit?.plays ?? 0, all: hit?.all ?? 0, col: Math.floor(i / 7), row: i % 7 });
		}
		return out;
	})();
	/** A month's name over the first week column it begins in. */
	$: calendarMonths = ((): { col: number; label: string }[] => {
		const out: { col: number; label: string }[] = [];
		let last = '';
		let lastCol = -10;
		for (const c of calendar) {
			const month = c.date.slice(0, 7);
			if (c.date < d.daily[0].date || month === last) continue;
			last = month;
			// A label is wider than the weeks of a short first month.
			if (c.col - lastCol < 4) continue;
			lastCol = c.col;
			const { y, m } = iso(c.date);
			out.push({ col: c.col, label: m === 1 || !out.length ? `${MONTHS[m - 1]} ${y}` : MONTHS[m - 1] });
		}
		return out;
	})();
	$: calendarWeeks = calendar.length ? calendar[calendar.length - 1].col + 1 : 0;
	$: weekRows = Array.from({ length: 7 }, (_, i) => (d.week_start + i) % 7);

	function heat(n: number, max: number): string {
		if (!n) return 'var(--td-surface2)';
		return `color-mix(in srgb, var(--td-accent) ${Math.round(20 + 80 * (n / max))}%, var(--td-surface2))`;
	}

	// ── Monthly share ──
	$: monthShares = d.months.map((m) => ({ ...m, share: m.all ? m.plays / m.all : 0 }));
	$: maxShare = Math.max(0.0001, ...monthShares.map((m) => m.share));

	/** Open a long timeline at its newest end, where the reader starts. The
	 *  parameter is only there so a change of data re-runs it. */
	// eslint-disable-next-line @typescript-eslint/no-unused-vars
	function atEnd(node: HTMLElement, _length: number) {
		const go = () => (node.scrollLeft = node.scrollWidth);
		go();
		return { update: go };
	}

	// ── Clock ──
	$: maxClock = Math.max(1, ...d.clock.flat());
</script>

<section class="dossier">
	<h3 class="h-display" style="font-size:20px;margin:0 0 4px;">Your history with {name}</h3>
	<p class="lede">
		{plural(d.plays, 'listen')} — {pct(d.plays, d.all_plays)} of everything you played{#if d.capped}, over the most recent part of your history this server reads for analytics{/if}.
	</p>

	<StatStrip stats={cards} />

	{#if d.habit}
		<div class="panel card">
			<div class="habit-head">
				<h4 class="h-display">{verdict}</h4>
				<span class="muted">score {Math.round(d.habit.score)} of 100</span>
			</div>
			<div class="meter" aria-hidden="true"><div style="width:{d.habit.score}%"></div></div>
			<p class="muted" style="margin:10px 0 0;">
				Played on {pct(d.habit.return_rate, 1)} of the days between your first and last listen, which span
				{pct(d.habit.span_coverage, 1)} of your history. Their biggest day held {pct(d.habit.top_day_share, 1)} of their listens.
			</p>
		</div>
	{/if}

	{#if calendar.length}
		<div class="panel card">
			<h4 class="h-display">Every day you played them</h4>
			<p class="muted" style="margin:0 0 12px;">
				Darker is more listens.{#if d.playthroughs.length} Outlined days are album playthroughs.{/if}
			</p>
			<div class="scroll" use:atEnd={calendarWeeks}>
				<div class="cal" style="--weeks:{calendarWeeks};">
					<div class="cal-months">
						{#each calendarMonths as m}<span style="grid-column:{m.col + 1};">{m.label}</span>{/each}
					</div>
					<div class="cal-body">
						<div class="cal-days">
							{#each weekRows as r, i}<span style="grid-row:{i + 1};">{i % 2 === 0 ? DAYS[r] : ''}</span>{/each}
						</div>
						<div class="cal-grid">
							{#each calendar as c (c.date)}
								<span
									class="cal-cell"
									class:through={playthroughDays.has(c.date)}
									style="grid-column:{c.col + 1};grid-row:{c.row + 1};background:{heat(c.plays, maxDay)};"
									title={c.plays ? `${longDate(c.date)}: ${c.plays} of ${c.all} listens` : longDate(c.date)}
								></span>
							{/each}
						</div>
					</div>
				</div>
			</div>
		</div>
	{/if}

	{#if monthShares.length > 1}
		<div class="panel card">
			<h4 class="h-display">Their share of each month</h4>
			<p class="muted" style="margin:0 0 12px;">Of everything you played that month.</p>
			<div class="scroll" use:atEnd={monthShares.length}>
				<div class="months" style="min-width:{monthShares.length * 14}px;">
					{#each monthShares as m (m.month)}
						{@const at = iso(`${m.month}-01`)}
						<div class="mcol" title={`${MONTHS[at.m - 1]} ${at.y}: ${m.plays} of ${m.all} listens (${pct(m.plays, m.all)})`}>
							<div class="mtrack"><div class="mfill" style="height:{(m.share / maxShare) * 100}%"></div></div>
							<span class="mlabel">{at.m === 1 || m === monthShares[0] ? at.y : ''}</span>
						</div>
					{/each}
				</div>
			</div>
		</div>
	{/if}

	{#if d.plays >= 10}
		<div class="panel card">
			<h4 class="h-display">When you play them</h4>
			<div class="scroll">
				<div class="heat">
					<div class="heat-row heat-head">
						<span></span>
						{#each Array(24) as _, h}<span class="heat-hour">{h % 6 === 0 ? h : ''}</span>{/each}
					</div>
					{#each weekRows as r}
						<div class="heat-row">
							<span class="heat-day">{DAYS[r]}</span>
							{#each d.clock[r] as c, h}
								<span class="heat-cell" title={`${DAYS[r]} ${h}:00 — ${c} listens`} style="background:{heat(c, maxClock)};"></span>
							{/each}
						</div>
					{/each}
				</div>
			</div>
		</div>
	{/if}

	{#if d.top_days.length || d.runs.length || d.playthroughs.length}
		<div class="lists">
			{#if d.top_days.length}
				<div class="panel card">
					<h4 class="h-display">Biggest days</h4>
					{#each d.top_days as t (t.date)}
						<div class="row">
							<span>{longDate(t.date)}</span>
							<span class="muted">{t.plays} of {t.all} ({pct(t.plays, t.all)})</span>
						</div>
					{/each}
				</div>
			{/if}
			{#if d.runs.length}
				<div class="panel card">
					<h4 class="h-display">Longest runs</h4>
					<p class="muted" style="margin:0 0 8px;">Nobody else in between, no half-hour silence.</p>
					{#each d.runs as r (r.start)}
						<div class="row">
							<span>{r.length} in a row</span>
							<span class="muted">{formatDateTime(r.start)}</span>
						</div>
					{/each}
				</div>
			{/if}
			{#if d.playthroughs.length}
				<div class="panel card">
					<h4 class="h-display">Album playthroughs</h4>
					<p class="muted" style="margin:0 0 8px;">Four or more tracks in order, in one sitting.</p>
					{#each d.playthroughs as p (p.start)}
						<div class="row">
							<span class="clip">{p.album}</span>
							<span class="muted">tracks {p.first_track}–{p.last_track} · {longDate(p.date)}</span>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	{/if}
</section>

<style>
	.dossier { margin-bottom: 30px; }
	.lede { font-size: 13px; color: var(--td-muted); margin: 0 0 14px; }
	.card { border-radius: 22px; padding: 20px 22px; margin-bottom: 14px; }
	.card h4 { font-size: 17px; margin: 0 0 4px; }
	.muted { font-size: 12.5px; color: var(--td-muted-2); }
	.scroll { overflow-x: auto; overflow-y: hidden; }

	.habit-head { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }
	.meter { height: 8px; border-radius: 999px; background: var(--td-surface2); margin-top: 8px; overflow: hidden; }
	.meter div { height: 100%; border-radius: 999px; background: var(--td-accent); }

	.cal { display: inline-block; }
	.cal-months {
		display: grid;
		grid-template-columns: repeat(var(--weeks), 11px);
		column-gap: 3px;
		margin-left: 30px;
		font-size: 10px;
		color: var(--td-muted-3);
		height: 14px;
		white-space: nowrap;
	}
	.cal-body { display: flex; gap: 4px; }
	.cal-days {
		display: grid;
		grid-template-rows: repeat(7, 11px);
		row-gap: 3px;
		width: 26px;
		font-size: 9.5px;
		color: var(--td-muted-3);
		line-height: 11px;
	}
	.cal-grid {
		display: grid;
		grid-template-columns: repeat(var(--weeks), 11px);
		grid-template-rows: repeat(7, 11px);
		gap: 3px;
	}
	.cal-cell { border-radius: 2.5px; }
	.cal-cell.through { box-shadow: inset 0 0 0 1.5px var(--td-text); }

	.months { display: flex; align-items: flex-end; gap: 3px; height: 130px; }
	.mcol { flex: 1; min-width: 8px; display: flex; flex-direction: column; height: 100%; }
	.mtrack { flex: 1; display: flex; align-items: flex-end; }
	.mfill { width: 100%; min-height: 1px; border-radius: 3px 3px 0 0; background: var(--td-accent); }
	.mlabel { font-size: 9.5px; color: var(--td-muted-3); height: 14px; margin-top: 4px; white-space: nowrap; }

	.heat { min-width: 560px; }
	.heat-row {
		display: grid;
		grid-template-columns: 38px repeat(24, minmax(0, 1fr));
		gap: 4px;
		align-items: center;
		margin-bottom: 4px;
	}
	.heat-head { margin-bottom: 6px; }
	.heat-hour { font-size: 9px; color: var(--td-muted-3); text-align: center; }
	.heat-day { font-size: 11px; color: var(--td-muted-2); }
	.heat-cell { aspect-ratio: 1; max-height: 22px; border-radius: 4px; }

	.lists { display: grid; grid-template-columns: repeat(auto-fit, minmax(260px, 1fr)); gap: 14px; }
	.lists .card { margin-bottom: 0; }
	.row { display: flex; justify-content: space-between; gap: 12px; padding: 7px 0; border-top: 1px solid var(--td-border); font-size: 13.5px; }
	.row:first-of-type { border-top: none; }
	.clip { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
