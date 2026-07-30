<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type RangedStats, type StatsRange, type Personality } from '$lib/api';
	import CoverArt from '$lib/components/CoverArt.svelte';

	const ranges: { v: StatsRange; label: string }[] = [
		{ v: 'week', label: 'Week' },
		{ v: 'month', label: 'Month' },
		{ v: 'last30', label: 'Last 30' },
		{ v: 'year', label: 'Year' },
		{ v: 'all', label: 'All' }
	];
	let range: StatsRange = 'all';
	let stats: RangedStats | null = null;
	let persona: Personality | null = null;
	let loading = true;
	let metric: 'plays' | 'hours' = 'plays';

	const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

	$: maxArtist = stats ? Math.max(1, ...stats.top_artists.map((a) => (metric === 'plays' ? a.plays : a.hours))) : 1;
	$: maxAlbum = stats ? Math.max(1, ...stats.top_albums.map((a) => (metric === 'plays' ? a.plays : a.hours))) : 1;
	$: maxDecade = stats ? Math.max(1, ...stats.decades.map((d) => d.plays)) : 1;
	$: maxHeat = stats ? Math.max(1, ...stats.heatmap.flat()) : 1;

	// Match the quality-badge colours so a tier reads the same everywhere.
	const fidelityFill = (tier: string) =>
		({
			DSD: 'linear-gradient(90deg,#c67139,#e07a55)',
			Lossless: 'linear-gradient(90deg,#7a8a5e,#aebf92)',
			Lossy: 'linear-gradient(90deg,#8a8172,#b3a487)'
		})[tier] ?? 'linear-gradient(90deg,#645c50,#9a8f7d)';

	async function load() {
		loading = true;
		try {
			[stats, persona] = await Promise.all([api.getRangedStats(range), api.getPersonality(range)]);
		} catch (e) { console.warn('stats', e); }
		loading = false;
	}

	function setRange(r: StatsRange) { range = r; load(); }

	onMount(load);
</script>

<svelte:head><title>Analytics — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;flex-wrap:wrap;margin-bottom:20px;">
	<div>
		<h1 class="h-display" style="font-size:38px;">Analytics</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">The shape of your listening.</p>
	</div>
	<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:4px;gap:2px;">
		{#each ranges as r}
			<button class="seg-btn" class:on={range === r.v} on:click={() => setRange(r.v)}>{r.label}</button>
		{/each}
	</div>
</div>

{#if loading && !stats}
	<div style="display:flex;justify-content:center;padding:80px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else if stats}
	<!-- Musical personality -->
	{#if persona && persona.traits.length}
		<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:24px;background:linear-gradient(135deg, var(--td-panel), var(--td-panel2));color:#f4e7cf;">
			<div style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:#c9a678;margin-bottom:8px;">Your musical personality</div>
			<div class="h-display" style="font-size:32px;margin-bottom:20px;color:#f4e7cf;">{persona.verdict}</div>
			<div style="display:flex;flex-direction:column;gap:16px;">
				{#each persona.traits as tr}
					<div>
						<div style="display:flex;justify-content:space-between;font-size:12.5px;margin-bottom:6px;color:#e9dcc4;">
							<span style="opacity:{tr.score < 50 ? 1 : .5};font-weight:600;">{tr.low}</span>
							<span style="color:#c9a678;">{tr.evidence}</span>
							<span style="opacity:{tr.score > 50 ? 1 : .5};font-weight:600;">{tr.high}</span>
						</div>
						<div style="position:relative;height:8px;border-radius:999px;background:rgba(244,231,207,.14);">
							<div style="position:absolute;top:50%;left:50%;width:1px;height:14px;transform:translate(-50%,-50%);background:rgba(244,231,207,.3);"></div>
							<div style="position:absolute;top:-3px;width:14px;height:14px;border-radius:50%;background:var(--td-accent-hi);box-shadow:0 0 0 3px rgba(224,122,85,.25);left:calc({tr.score}% - 7px);"></div>
						</div>
					</div>
				{/each}
			</div>
			{#if persona.loyalty.length}
				<div style="margin-top:22px;padding-top:16px;border-top:1px solid rgba(244,231,207,.14);">
					<div style="font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:#c9a678;margin-bottom:10px;">Most loyal to</div>
					<div style="display:flex;gap:8px;flex-wrap:wrap;">
						{#each persona.loyalty.slice(0, 6) as l}
							<span class="tag" style="background:rgba(244,231,207,.1);color:#e9dcc4;border:none;">{l.artist} · {Math.round(l.loyalty_score)}</span>
						{/each}
					</div>
				</div>
			{/if}
		</div>
	{/if}

	{@const t = stats.totals}
	<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:14px;margin-bottom:14px;">
		{#each [['Total Plays', t.plays], ['Artists', t.artists], ['Albums', t.albums], ['Tracks', t.tracks]] as [label, val]}
			<div class="panel" style="border-radius:20px;padding:18px 20px;">
				<div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">{label}</div>
				<div class="h-display" style="font-size:30px;line-height:1;">{val}</div>
			</div>
		{/each}
	</div>
	<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(140px,1fr));gap:14px;margin-bottom:28px;">
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Hours</div><div class="h-display" style="font-size:30px;line-height:1;">{t.hours}</div></div>
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Avg Quality</div><div class="h-display" style="font-size:30px;line-height:1;">{t.avg_quality}<span style="font-size:16px;color:var(--td-muted-2);">/100</span></div></div>
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Lossless</div><div class="h-display" style="font-size:30px;line-height:1;color:var(--td-good-fg);">{t.lossless_pct}%</div></div>
		<div class="panel" style="border-radius:20px;padding:18px 20px;"><div style="font-size:10.5px;letter-spacing:.1em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">DSD</div><div class="h-display" style="font-size:30px;line-height:1;color:#b2622d;">{t.dsd_pct}%</div></div>
	</div>

	<!-- Top artists -->
	<div class="panel" style="border-radius:22px;padding:22px;margin-bottom:16px;">
		<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:16px;">
			<h3 class="h-display" style="font-size:19px;">Top Artists</h3>
			<div style="display:inline-flex;background:var(--td-sidebar);border-radius:999px;padding:3px;gap:2px;">
				<button class="seg-btn" class:on={metric === 'plays'} on:click={() => (metric = 'plays')}>Plays</button>
				<button class="seg-btn" class:on={metric === 'hours'} on:click={() => (metric = 'hours')}>Hours</button>
			</div>
		</div>
		{#if stats.top_artists.length}
			{#each stats.top_artists as a}
				{@const v = metric === 'plays' ? a.plays : a.hours}
				<div style="display:flex;align-items:center;gap:12px;margin-bottom:13px;">
					<CoverArt url={a.image_url} seed={a.name} size={34} radius={999} />
					<div style="flex:1;min-width:0;">
						<div style="display:flex;justify-content:space-between;font-size:13px;margin-bottom:5px;"><span style="font-weight:600;">{a.name}</span><span style="color:var(--td-muted-2);">{v}{metric === 'hours' ? 'h' : ''}</span></div>
						<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{(v / maxArtist * 100).toFixed(0)}%;height:100%;background:linear-gradient(90deg,#c67139,#e07a55);border-radius:999px;"></div></div>
					</div>
				</div>
			{/each}
		{:else}
			<p style="color:var(--td-muted);font-size:14px;margin:0;">No plays in this range yet.</p>
		{/if}
	</div>

	<!-- Top albums -->
	<div class="panel" style="border-radius:22px;padding:22px;margin-bottom:16px;">
		<h3 class="h-display" style="font-size:19px;margin:0 0 16px;">Top Albums</h3>
		{#if stats.top_albums.length}
			{#each stats.top_albums as al}
				{@const v = metric === 'plays' ? al.plays : al.hours}
				<div style="margin-bottom:13px;">
					<div style="display:flex;justify-content:space-between;gap:12px;font-size:13px;margin-bottom:5px;">
						<span style="min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">
							<span style="font-weight:600;">{al.name}</span>
							<span style="color:var(--td-muted-3);"> — {al.artist}</span>
						</span>
						<span style="color:var(--td-muted-2);flex:none;">{v}{metric === 'hours' ? 'h' : ''}</span>
					</div>
					<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{(v / maxAlbum * 100).toFixed(0)}%;height:100%;background:linear-gradient(90deg,#7a8a5e,#aebf92);border-radius:999px;"></div></div>
				</div>
			{/each}
		{:else}
			<p style="color:var(--td-muted);font-size:14px;margin:0;">No album data in this range yet.</p>
		{/if}
	</div>

	<!-- Fidelity mix -->
	{#if stats.fidelity.length}
		<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Fidelity Mix</h3>
		<div class="panel" style="border-radius:22px;padding:22px;margin-bottom:16px;display:flex;flex-direction:column;gap:14px;">
			{#each stats.fidelity as f}
				<div>
					<div style="display:flex;justify-content:space-between;font-size:13px;margin-bottom:6px;">
						<span style="font-weight:600;">{f.name}</span>
						<span style="color:var(--td-muted-2);">{f.pct}% · {f.plays}</span>
					</div>
					<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{f.pct}%;height:100%;background:{fidelityFill(f.name)};border-radius:999px;"></div></div>
				</div>
			{/each}
		</div>
	{/if}

	<!-- Decades -->
	{#if stats.decades.length}
		<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">By Decade</h3>
		<div class="panel" style="border-radius:22px;padding:22px;margin-bottom:16px;overflow-x:auto;">
			<div style="display:flex;align-items:flex-end;gap:16px;height:150px;min-width:{stats.decades.length * 60}px;">
				{#each stats.decades as d}
					<div style="flex:1;display:flex;flex-direction:column;align-items:center;justify-content:flex-end;height:100%;gap:8px;">
						<span style="font-size:12px;color:var(--td-muted-2);">{d.plays}</span>
						<div style="width:100%;background:var(--td-track);border-radius:12px;overflow:hidden;display:flex;align-items:flex-end;height:100%;">
							<div style="width:100%;height:{Math.max(4, (d.plays / maxDecade) * 100)}%;background:linear-gradient(180deg,#e07a55,#c67139);border-radius:12px;"></div>
						</div>
						<span style="font-size:12.5px;font-weight:600;">{d.name}</span>
					</div>
				{/each}
			</div>
		</div>
	{/if}

	<!-- Heatmap -->
	<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Listening Clock</h3>
	<div class="panel" style="border-radius:22px;padding:22px;overflow-x:auto;">
		<div style="min-width:560px;">
			<div style="display:flex;margin-left:38px;margin-bottom:6px;">
				{#each Array(24) as _, h}<span style="flex:1;text-align:center;font-size:9px;color:var(--td-muted-3);">{h % 6 === 0 ? h : ''}</span>{/each}
			</div>
			{#each stats.heatmap as row, di}
				<div style="display:flex;align-items:center;gap:3px;margin-bottom:3px;">
					<span style="width:34px;font-size:10.5px;color:var(--td-muted-3);text-align:right;padding-right:6px;">{days[di]}</span>
					{#each row as c}
						<span title={`${c} plays`} style="flex:1;aspect-ratio:1;max-width:20px;border-radius:3px;background:rgba(198,113,57,{c === 0 ? 0.06 : 0.15 + 0.8 * (c / maxHeat)});"></span>
					{/each}
				</div>
			{/each}
			<div style="display:flex;align-items:center;gap:6px;margin-top:14px;margin-left:38px;font-size:10.5px;color:var(--td-muted-3);">Less
				<span style="width:13px;height:13px;border-radius:3px;background:rgba(198,113,57,.12);"></span>
				<span style="width:13px;height:13px;border-radius:3px;background:rgba(198,113,57,.38);"></span>
				<span style="width:13px;height:13px;border-radius:3px;background:rgba(198,113,57,.64);"></span>
				<span style="width:13px;height:13px;border-radius:3px;background:rgba(198,113,57,.92);"></span>More</div>
		</div>
	</div>
	<!-- Languages -->
	{#if stats.languages.length}
		<h3 class="h-display" style="font-size:19px;margin:24px 0 12px;">Languages</h3>
		<div class="panel" style="border-radius:22px;padding:22px;">
			{#each stats.languages as l}
				<div style="margin-bottom:12px;">
					<div style="display:flex;justify-content:space-between;font-size:13px;margin-bottom:5px;"><span style="font-weight:600;">{l.name}</span><span style="color:var(--td-muted-2);">{l.pct}%</span></div>
					<div style="height:10px;border-radius:999px;background:var(--td-track);overflow:hidden;"><div style="width:{l.pct}%;height:100%;background:linear-gradient(90deg,#7a8a5e,#aebf92);border-radius:999px;"></div></div>
				</div>
			{/each}
		</div>
	{:else}
		<p style="color:var(--td-muted-3);font-size:12.5px;margin:16px 0 0;">Language breakdown fills in as listens are classified (Settings → run enrichment).</p>
	{/if}
{/if}

<style>
	.spin { animation: td-spin 0.7s linear infinite; }
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
</style>
