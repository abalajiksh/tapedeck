<script lang="ts">
	/**
	 * Reports — a feed of period cards, newest first.
	 *
	 * Its own surface rather than a tab under Statistics (Ashwin's call). The
	 * cost is named in the roadmap: this is a second analytics home whose period
	 * control can drift out of step with Statistics and Top Charts. Organising
	 * the four of them is deliberately later work.
	 *
	 * House rules held from the design package: no play buttons (Tapedeck has no
	 * playback), the genre split is labelled a share of tag mentions, and
	 * nothing here points at music you don't own.
	 */
	import { onMount } from 'svelte';
	import { syncUrl } from '$lib/url';
	import { api } from '$lib/api';
	import type { Reports, ReportPeriod } from '$lib/api';
	import { openArtist, openAlbum } from '$lib/entities';
	import ToTop from '$lib/components/ToTop.svelte';
	import WeekChapter from '$lib/components/reports/WeekChapter.svelte';
	import MonthChapter from '$lib/components/reports/MonthChapter.svelte';
	import YearChapter from '$lib/components/reports/YearChapter.svelte';
	import PeriodNote from '$lib/components/reports/PeriodNote.svelte';

	const UNITS: { k: 'week' | 'month' | 'year'; label: string }[] = [
		{ k: 'week', label: 'Week' },
		{ k: 'month', label: 'Month' },
		{ k: 'year', label: 'Year' }
	];

	let unit: 'week' | 'month' | 'year' = 'month';
	/** Which period is on screen, counting back from the newest. */
	let idx = 0;
	let data: Reports | null = null;
	let loading = true;
	let error = '';

	/** One chapter at a time, so stepping back and forth doesn't refetch. Keyed
	 *  by unit and offset; cleared when the unit changes since the buckets are
	 *  a different shape. */
	let cache = new Map<string, Reports>();

	$: total = data?.total_periods ?? 0;
	$: canOlder = idx + 1 < total;
	$: canNewer = idx > 0;
	$: period = data?.periods[0] ?? null;

	async function load() {
		const key = `${unit}:${idx}`;
		const hit = cache.get(key);
		if (hit) {
			data = hit;
			error = '';
			updateUrl();
			return;
		}
		loading = true;
		try {
			// One period per request. The server folds the whole history on every
			// call regardless, so a page is a slice of work already done.
			const r = await api.getReports(unit, 1, idx);
			cache.set(key, r);
			data = r;
			error = '';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
		updateUrl();
	}

	function updateUrl() {
		syncUrl(`/reports?unit=${unit}${idx ? `&at=${idx}` : ''}`);
	}

	function setUnit(u: 'week' | 'month' | 'year') {
		if (u === unit) return;
		unit = u;
		// Back to the newest: index 3 of "week" is not index 3 of "year", so
		// carrying it over lands somewhere arbitrary.
		idx = 0;
		load();
	}

	function step(by: number) {
		const next = idx + by;
		if (next < 0 || next >= total) return;
		idx = next;
		load();
	}

	/** ← / → walk the chapters, which is what a stepper invites you to try.
	 *  Ignored while typing so it can't fight a search field added later. */
	function onKey(e: KeyboardEvent) {
		const t = e.target as HTMLElement | null;
		if (t && /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName)) return;
		if (e.key === 'ArrowLeft') step(1);
		else if (e.key === 'ArrowRight') step(-1);
	}

	/** A signed change against the previous period, or null when there isn't one. */
	function delta(now: number, before: number | undefined | null) {
		if (before == null || before === 0) return null;
		return Math.round(((now - before) / before) * 100);
	}

	const nf = (n: number) => n.toLocaleString();

	/**
	 * One line of editorial, built only from what the data actually says.
	 *
	 * Deliberately conservative: it names the top artist and, when there is one,
	 * the discovery. No adjectives the numbers don't support — a generated
	 * sentence claiming a "loud, loyal month" from a play count is the kind of
	 * thing that reads well once and wrong every time after.
	 */
	function blurb(p: ReportPeriod): string {
		const parts: string[] = [];
		if (p.top_artist) parts.push(`Mostly ${p.top_artist.name}`);
		if (p.crush) parts.push(`and you found ${p.crush.name}`);
		const d = delta(p.plays, p.prev?.plays);
		if (d != null && Math.abs(d) >= 15) {
			parts.push(`— ${d > 0 ? 'up' : 'down'} ${Math.abs(d)}% on ${p.prev?.label}`);
		}
		return parts.join(' ');
	}

	/** The one line of provenance the design's footer carries. Kept because it
	 *  is true and load-bearing: nothing on this page is a new subsystem. */
	$: footNote = `Folded from one pass over your history, bucketed in ${data?.timezone?.replace(/_/g, ' ') ?? 'your zone'}.`;

	onMount(() => {
		const qs = new URLSearchParams(window.location.search);
		const q = qs.get('unit');
		if (q === 'week' || q === 'month' || q === 'year') unit = q;
		const at = Number(qs.get('at'));
		if (Number.isInteger(at) && at > 0) idx = at;
		load();
	});
</script>

<svelte:head><title>Reports — Tapedeck</title></svelte:head>
<svelte:window on:keydown={onKey} />

<div class="page-head">
	<div style="min-width:0;">
		<div class="eyebrow">Reports</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">Your listening, in chapters</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">
			One chapter per period, bucketed in your own timezone{data?.timezone
				? ` (${data.timezone.replace(/_/g, ' ')})`
				: ''}.
		</p>
	</div>
	<div class="seg">
		{#each UNITS as u}
			<button class="seg-btn" class:on={unit === u.k} on:click={() => setUnit(u.k)}>{u.label}</button>
		{/each}
	</div>
</div>

<!-- The stepper. This used to be a feed of twelve stacked cards, which meant
     the far end of the history was unreachable and every visit rendered eleven
     chapters nobody asked for. -->
{#if total > 0}
	<div class="stepper">
		<button class="step" disabled={!canOlder} on:click={() => step(1)} aria-label="Older period">
			<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M15 6l-6 6 6 6"/></svg>
		</button>
		<span class="h-display step-label">{period?.label ?? '—'}</span>
		<button class="step" disabled={!canNewer} on:click={() => step(-1)} aria-label="Newer period">
			<svg viewBox="0 0 24 24" width="15" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>
		</button>
	</div>
	<p class="step-of">{idx + 1} of {total} {unit === 'week' ? 'weeks' : unit === 'month' ? 'months' : 'years'}</p>
{/if}

{#if loading && !data}
	<div style="display:flex;justify-content:center;padding:80px;">
		<div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div>
	</div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:24px;">
		<p style="font-size:13.5px;color:var(--td-muted);margin:0;">{error}</p>
	</div>
{:else if !data?.periods.length}
	<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">
		Nothing to report yet — press play on something.
	</div>
{:else}
	{#if data.capped}
		<p style="font-size:12px;color:var(--td-muted-3);margin:0 0 14px;">
			Your history is longer than the analytics cap, so the oldest periods here are partial.
		</p>
	{/if}

	{#if period}
		{#key period.key}
			{#if unit === 'year'}
				<YearChapter p={period} onShare={null} />
			{:else if unit === 'week'}
				<WeekChapter p={period} onShare={null}>
					<svelte:fragment slot="blurb">{blurb(period)}</svelte:fragment>
					<svelte:fragment slot="footer">
						<div class="chapter-foot"><p class="foot-note">{footNote}</p></div>
					</svelte:fragment>
				</WeekChapter>
			{:else}
				<MonthChapter p={period} onShare={null}>
					<svelte:fragment slot="blurb">{blurb(period)}</svelte:fragment>
					<svelte:fragment slot="footer">
						<div class="chapter-foot"><p class="foot-note">{footNote}</p></div>
					</svelte:fragment>
				</MonthChapter>
			{/if}
		{/key}

		<!-- Below the chapter, not inside one of the three layouts: a note is the
		     same thing whichever unit is on screen, and putting it in each layout
		     would be three copies free to drift. Keyed on the period so stepping
		     to another one cannot leave the previous note's editor open over it. -->
		{#key `${unit}:${period.key}`}
			<PeriodNote {unit} periodKey={period.key} label={period.label} />
		{/key}
	{/if}

	<ToTop />
{/if}

<style>
	/* Same segmented control as Statistics and Top Charts. Svelte scopes styles
	   per component, so this is repeated rather than inherited. */
	.seg-btn {
		border: none; background: transparent; cursor: pointer;
		color: var(--td-nav-fg); font-size: 12.5px; padding: 6px 14px;
		border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	.eyebrow {
		font-size: 11px; letter-spacing: 0.16em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 6px;
	}
	.page-head {
		display: flex; align-items: flex-end; justify-content: space-between;
		gap: 16px; flex-wrap: wrap; margin-bottom: 18px;
	}
	.seg {
		display: inline-flex; background: var(--td-sidebar);
		border-radius: 999px; padding: 4px; gap: 2px;
	}

	/* One chapter at a time. Capped in width so it reads as a control rather
	   than a header band stretched across the page. */
	.stepper {
		display: flex; align-items: center; justify-content: space-between;
		gap: 12px; background: var(--td-surface); border: 1px solid var(--td-border);
		border-radius: 999px; padding: 6px 10px; max-width: 340px;
	}
	.step {
		width: 32px; height: 32px; flex: none; border-radius: 50%; border: none;
		background: var(--td-sidebar); color: var(--td-nav-fg); cursor: pointer;
		display: flex; align-items: center; justify-content: center;
	}
	.step:hover:not(:disabled) { background: var(--td-nav-hover); color: var(--td-text); }
	.step:disabled { opacity: 0.4; cursor: default; }
	.step-label { font-size: 16px; text-align: center; flex: 1; min-width: 0; }
	.step-of { font-size: 12px; color: var(--td-muted-3); margin: 8px 0 18px 14px; }

	/* The chapter footer, rendered into each layout's footer slot. */
	.chapter-foot { border-top: 1px solid var(--td-border); padding-top: 18px; margin-top: 4px; }
	.foot-note { font-size: 12px; color: var(--td-muted-3); margin: 0; max-width: 52ch; }

	.spin {
		width: 22px; height: 22px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		animation: td-spin 0.7s linear infinite;
	}
</style>
