<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api';
	import type { Scrobble } from '$lib/api';
	import ScrobbleRow from '$lib/components/ScrobbleRow.svelte';
	import { qualityLabel, tierTagClass } from '$lib/utils';

	let recent: Scrobble[] = [];
	let loading = true;
	let connected = false;
	let today = 0, thisWeek = 0, losslessPct = 0;

	$: hero = recent[0] ?? null;
	$: heroQ = hero ? qualityLabel(hero) : null;

	onMount(async () => {
		try {
			connected = (await api.health()).status === 'ok';
		} catch { connected = false; }

		try {
			const s = await api.getDashboardStats();
			today = s.today; thisWeek = s.this_week; losslessPct = s.lossless_pct;
		} catch (e) { console.warn('stats', e); }

		try {
			recent = (await api.getScrobbles({ limit: 8 })).scrobbles;
		} catch (e) { console.warn('scrobbles', e); }

		loading = false;
	});
</script>

<svelte:head><title>Dashboard — Tapedeck</title></svelte:head>

<div style="display:flex;align-items:flex-end;justify-content:space-between;gap:16px;margin-bottom:24px;">
	<div style="flex:1;min-width:0;">
		<h1 class="h-display" style="font-size:38px;">Now Rolling</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">Your listening, one side at a time.</p>
	</div>
	<span style="display:inline-flex;align-items:center;gap:8px;font-size:12.5px;color:var(--td-good-fg);background:var(--td-good-bg);padding:7px 14px;border-radius:999px;">
		<span style="width:7px;height:7px;border-radius:50%;background:{connected ? '#7a8a5e' : '#b2622d'};"></span>{connected ? 'Connected' : 'Offline'}</span>
</div>

{#if loading}
	<div style="display:flex;justify-content:center;padding:80px;"><div class="spin" style="width:22px;height:22px;border:2px solid var(--td-track);border-top-color:var(--td-accent);border-radius:50%;"></div></div>
{:else}
	<!-- Cassette hero -->
	<div class="panel" style="border-radius:28px;box-shadow:0 10px 30px rgba(46,43,37,.09);padding:24px;margin-bottom:22px;display:flex;gap:28px;flex-wrap:wrap;align-items:center;">
		<div style="width:340px;max-width:100%;flex:none;">
			<svg viewBox="0 0 340 210" style="width:100%;height:auto;display:block;">
				<rect x="6" y="6" width="328" height="198" rx="22" fill="#2b2721"/>
				<rect x="6" y="6" width="328" height="198" rx="22" fill="none" stroke="#c67139" stroke-width="1.6" opacity=".55"/>
				<rect x="28" y="20" width="284" height="50" rx="10" fill="#f4e7cf"/>
				<line x1="40" y1="33" x2="300" y2="33" stroke="#c67139" stroke-width="3" opacity=".55"/>
				<line x1="40" y1="45" x2="240" y2="45" stroke="#7a8a5e" stroke-width="3" opacity=".45"/>
				<rect x="62" y="88" width="216" height="92" rx="12" fill="#1b1712"/>
				<rect x="120" y="120" width="100" height="28" rx="3" fill="#3a3128"/>
				{#each [120, 220] as cx}
					<g class="reel" style="transform-box:fill-box;transform-origin:center;">
						<circle {cx} cy="134" r="33" fill="none" stroke="#e9dcc4" stroke-width="3"/>
						<circle {cx} cy="134" r="22" fill="none" stroke="#7a6f5c" stroke-width="2" opacity=".6"/>
						<circle {cx} cy="134" r="8" fill="#e9dcc4"/>
						<line x1={cx - 30} y1="134" x2={cx + 30} y2="134" stroke="#e9dcc4" stroke-width="2.4"/>
						<line x1={cx - 15} y1="108" x2={cx + 15} y2="160" stroke="#e9dcc4" stroke-width="2.4"/>
						<line x1={cx - 15} y1="160" x2={cx + 15} y2="108" stroke="#e9dcc4" stroke-width="2.4"/>
					</g>
				{/each}
			</svg>
		</div>
		<div style="flex:1;min-width:260px;">
			{#if hero}
				<div style="display:flex;align-items:center;gap:8px;margin-bottom:8px;">
					<span style="width:8px;height:8px;border-radius:50%;background:#e07a55;animation:td-blink 1.6s ease-in-out infinite;"></span>
					<span style="font-size:11px;letter-spacing:.16em;text-transform:uppercase;color:#c67139;font-weight:600;">Last Played</span>
				</div>
				<h2 class="h-display" style="font-size:28px;margin:0 0 4px;">{hero.title}</h2>
				<p style="margin:0 0 14px;color:var(--td-muted);font-size:15.5px;">{hero.artist}{hero.album ? ` — ${hero.album}` : ''}</p>
				<div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;">
					{#if heroQ?.text}<span class={tierTagClass(heroQ.tier)}>{heroQ.text}</span>{/if}
					{#if hero.listening_context}<span class="tag tag-neutral">{hero.listening_context}</span>{/if}
				</div>
			{:else}
				<h2 class="h-display" style="font-size:26px;margin:0 0 6px;">The deck is empty</h2>
				<p style="margin:0;color:var(--td-muted);font-size:15px;">Nothing recorded yet — <a href="/import">import a backup</a> or start scrobbling.</p>
			{/if}
		</div>
	</div>

	<!-- Quick stats -->
	<div style="display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:16px;margin-bottom:30px;">
		<div class="panel" style="border-radius:22px;padding:20px 22px;">
			<div style="font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">Today</div>
			<div class="h-display" style="font-size:36px;line-height:1;">{today}</div>
			<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">plays</div>
		</div>
		<div class="panel" style="border-radius:22px;padding:20px 22px;">
			<div style="font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:6px;">This Week</div>
			<div class="h-display" style="font-size:36px;line-height:1;">{thisWeek}</div>
			<div style="font-size:12.5px;color:var(--td-muted-2);margin-top:6px;">plays</div>
		</div>
		<div style="background:var(--td-panel);color:#f4e7cf;border-radius:22px;padding:20px 22px;">
			<div style="font-size:11px;letter-spacing:.12em;text-transform:uppercase;color:#c9a678;margin-bottom:6px;">Lossless</div>
			<div class="h-display" style="font-size:36px;line-height:1;">{losslessPct}<span style="font-size:20px;">%</span></div>
			<div style="font-size:12.5px;color:#b3a487;margin-top:6px;">of all listens</div>
		</div>
	</div>

	<!-- Recent -->
	<div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:12px;">
		<h3 class="h-display" style="font-size:20px;">Recent Listens</h3>
		<a href="/history" style="font-size:13.5px;">View all →</a>
	</div>
	{#if recent.length}
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each recent as s (s.id)}<ScrobbleRow scrobble={s} />{/each}
		</div>
	{:else}
		<div class="panel" style="border-radius:22px;padding:44px;text-align:center;color:var(--td-muted);">No listens recorded yet.</div>
	{/if}
{/if}

<style>
	.reel { animation: td-spin 4s linear infinite; }
	.spin { animation: td-spin 0.7s linear infinite; }
</style>
