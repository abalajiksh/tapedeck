<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api, type Rediscovery, type RediscoveryCollection, type RediscoveryRow } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import CollectionIcon from '$lib/components/CollectionIcon.svelte';
	import PushPlaylist from '$lib/components/PushPlaylist.svelte';

	let data: Rediscovery | null = null;
	let loading = true;
	let error = '';

	// "Not lately" is a judgement, not a constant — six months is a long gap for
	// someone who listens daily and no gap at all for someone who doesn't.
	const WINDOWS = [
		{ days: 90, label: '3 months' },
		{ days: 180, label: '6 months' },
		{ days: 365, label: 'a year' },
		{ days: 730, label: '2 years' }
	];
	let quietDays = 180;

	/** Which card is expanded. Collections are summaries first — the two sample
	 *  tracks are the hook, the full list is a click away. */
	let openKey: string | null = null;
	let panel: HTMLElement | null = null;

	/** Open a collection, or close the one that's open.
	 *
	 *  The list renders below the whole grid, so with seven cards it can open
	 *  off-screen — which reads as the button having done nothing. The card
	 *  keeps its accent outline while its list is up, so it stays obvious which
	 *  of the seven you're looking at once you're down there. */
	async function toggle(key: string) {
		openKey = openKey === key ? null : key;
		if (!openKey) return;
		await tick();
		panel?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
	}

	let gearId: number | null = null;
	let gearRows: RediscoveryRow[] = [];
	let gearLoading = false;

	async function load() {
		loading = true;
		try {
			data = await api.rediscovery(quietDays);
			error = '';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
	}

	async function loadGear(id: number) {
		gearId = id;
		gearLoading = true;
		gearRows = [];
		try {
			gearRows = (await api.neverOnGear(id)).rows;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		gearLoading = false;
	}

	/** A track row opens the record it's on — the rule every track listing in
	 *  the app follows — falling back to the artist when there's no album. */
	function openRow(r: RediscoveryRow) {
		if (r.album) return openAlbum(r.artist, r.album);
		return openArtist(r.artist);
	}

	/** A collection's rows as a playlist push wants them.
	 *
	 *  Names only, which is all these rows have — the push resolves them against
	 *  whichever server it is sent to. `name` is the track title here; the row
	 *  shape predates the playlist code and renaming it would touch every
	 *  Rediscovery query. */
	function toTracks(rows: RediscoveryRow[]) {
		return rows.map((r) => ({ title: r.name, artist: r.artist, album: r.album }));
	}

	/** The accent at low opacity, for the badge behind each icon. */
	function tint(hex: string, alpha: number): string {
		const n = parseInt(hex.replace('#', ''), 16);
		return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
	}

	onMount(load);
</script>

<svelte:head><title>Rediscovery — Tapedeck</title></svelte:head>

<div style="margin-bottom:22px;">
	<h1 class="h-display" style="font-size:38px;margin:0 0 6px;letter-spacing:-.02em;">Rediscovery</h1>
	<p style="margin:0;color:var(--td-muted);font-size:15px;">
		Smart collections that dig up what you've loved and lost — all of it from
		your own listening history.
	</p>
</div>

<div style="display:flex;gap:8px;align-items:center;margin-bottom:22px;flex-wrap:wrap;">
	<span style="font-size:12.5px;color:var(--td-muted-2);">Quiet for</span>
	{#each WINDOWS as w}
		<button class="chip" class:on={quietDays === w.days}
			on:click={() => { quietDays = w.days; load(); }}>{w.label}</button>
	{/each}
</div>

{#if error}<p style="color:#b2622d;font-size:14px;">{error}</p>{/if}

{#if loading}
	<p style="color:var(--td-muted);font-size:14px;">Looking through your history…</p>
{:else if data}
	{#if data.featured}
		<!-- The hero. Its record turns because this is the one thing on the page
		     asking to be picked up — the rest are a catalogue. -->
		<div class="hero" class:open={openKey === data.featured.key}
			style={openKey === data.featured.key
				? `box-shadow:0 0 0 3px ${tint(data.featured.accent, 0.4)};`
				: ''}>
			<div style="width:120px;height:120px;flex:none;">
				<svg viewBox="0 0 120 120" style="width:100%;height:auto;display:block;">
					<circle cx="60" cy="60" r="57" fill="#c67139" opacity=".14" />
					<g class="platter" style="transform-origin:60px 60px;">
						<circle cx="60" cy="60" r="52" fill="#0d0a07" />
						{#each [44, 35, 26] as r}
							<circle cx="60" cy="60" r={r} fill="none" stroke="#2a241d" stroke-width="1" />
						{/each}
						<circle cx="60" cy="60" r="19" fill={data.featured.accent} />
						<circle cx="60" cy="60" r="14" fill="none" stroke="#f4e7cf" stroke-width="1" opacity=".5" />
						<line x1="60" y1="45" x2="60" y2="52" stroke="#f4e7cf" stroke-width="1.6" stroke-linecap="round" />
						<circle cx="60" cy="60" r="2.6" fill="#f4e7cf" />
					</g>
				</svg>
			</div>
			<div style="flex:1;min-width:240px;">
				<div class="eyebrow">This week's rediscovery</div>
				<h2 class="h-display" style="font-size:27px;margin:0 0 6px;letter-spacing:-.015em;">
					{data.featured.title}
				</h2>
				<p style="margin:0 0 14px;color:#c9bfa8;font-size:14.5px;max-width:52ch;">
					{data.featured.blurb}
				</p>
				<div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap;margin-bottom:16px;font-size:12.5px;color:#b3a487;">
					<span class="pill" style="background:{data.featured.accent};">
						{data.featured.count} track{data.featured.count === 1 ? '' : 's'}
					</span>
					<span>{data.featured.stat}</span>
				</div>
				<div style="display:flex;gap:8px;flex-wrap:wrap;align-items:center;">
					<button class="btn btn-primary"
						on:click={() => data?.featured && toggle(data.featured.key)}>
						{openKey === data.featured.key ? 'Hide' : 'See all'}
					</button>
					<PushPlaylist tracks={toTracks(data.featured.rows)}
						title={data.featured.title} accent={data.featured.accent} />
				</div>
			</div>
		</div>
	{/if}

	<div class="cards">
		<!-- The hero is one of these, so it has to come out of the grid — whichever
		     collection has the most to say was being rendered twice. -->
		{#each data.collections.filter((c) => c.key !== data?.featured?.key) as c (c.key)}
			<div class="card" class:empty={!c.count} class:open={openKey === c.key}
				style={openKey === c.key
					? `border-color:${c.accent};box-shadow:0 0 0 3px ${tint(c.accent, 0.22)};`
					: ''}>
				<div style="display:flex;gap:14px;align-items:flex-start;margin-bottom:14px;">
					<div class="badge" style="background:{tint(c.accent, 0.15)};color:{c.accent};">
						<CollectionIcon name={c.key} />
					</div>
					<div style="flex:1;min-width:0;">
						<h3 class="h-display" style="font-size:17.5px;margin:0 0 4px;line-height:1.15;">{c.title}</h3>
						<p style="margin:0;color:var(--td-muted);font-size:13px;line-height:1.45;">{c.blurb}</p>
					</div>
				</div>

				<div style="display:flex;align-items:center;gap:9px;flex-wrap:wrap;margin-bottom:12px;">
					<span class="pill" style="background:{c.accent};">
						{c.count} {c.key === 'dusty' ? (c.count === 1 ? 'record' : 'records') : (c.count === 1 ? 'track' : 'tracks')}
					</span>
					<span style="font-size:12px;color:var(--td-muted-2);">{c.stat}</span>
				</div>

				{#if c.count}
					<!-- Two samples, as chips. Enough to tell you whether the
					     collection is worth opening, which is all a card owes you. -->
					<div style="display:flex;flex-direction:column;gap:6px;margin-bottom:16px;">
						{#each c.rows.slice(0, 2) as t}
							<span class="tchip">{t.name} · {t.artist}</span>
						{/each}
					</div>
					<div style="display:flex;gap:8px;margin-top:auto;flex-wrap:wrap;">
						<button class="btn btn-secondary" style="font-size:13px;padding:8px 15px;"
							aria-expanded={openKey === c.key} on:click={() => toggle(c.key)}>
							{openKey === c.key ? 'Hide' : `See all ${c.count}`}
						</button>
						<PushPlaylist tracks={toTracks(c.rows)} title={c.title} accent={c.accent} compact />
					</div>
				{:else}
					<p style="margin:0;font-size:12.5px;color:var(--td-muted-3);">
						Nothing here yet — which is its own kind of answer.
					</p>
				{/if}
			</div>
		{/each}
	</div>

	{#each data.collections.filter((c) => c.key === openKey) as c (c.key)}
		<div class="panel expanded" bind:this={panel}
			style="border-color:{c.accent};box-shadow:0 0 0 3px {tint(c.accent, 0.22)};">
			<div style="display:flex;justify-content:space-between;align-items:baseline;gap:12px;padding:16px 18px 10px;">
				<h3 class="h-display" style="font-size:19px;margin:0;">{c.title}</h3>
				<span style="display:flex;gap:12px;align-items:center;">
					<PushPlaylist tracks={toTracks(c.rows)} title={c.title} accent={c.accent} compact />
					<button class="linkish" on:click={() => (openKey = null)}>Close</button>
				</span>
			</div>
			{#each c.rows as r, i}
				<button class="row" on:click={() => openRow(r)}>
					<CoverArt url={r.artwork_url} caaId={r.caa_id} releaseMbid={r.caa_release_mbid}
						seed={`${r.artist} ${r.name}`} size={38} radius={8} />
					<span style="flex:1;min-width:0;text-align:left;">
						<span class="nm">{r.name}</span>
						<span class="sb">{r.artist}{r.album && r.album !== r.name ? ` — ${r.album}` : ''}</span>
					</span>
					<span class="meta">{r.plays} play{r.plays === 1 ? '' : 's'}</span>
				</button>
				{#if i < c.rows.length - 1}<div class="sep"></div>{/if}
			{/each}
		</div>
	{/each}

	<!-- Gear needs a choice made, so it can't be a card like the rest. -->
	<div style="margin-top:30px;">
		<h3 class="h-display" style="font-size:20px;margin:0 0 2px;">Never heard on…</h3>
		<p style="margin:0 0 12px;font-size:13px;color:var(--td-muted);">
			Favourites that have never been through a particular piece of gear. Comes
			from the signal chain on each listen — a chain names its components.
		</p>

		{#if data.gear.length}
			<div style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:12px;">
				{#each data.gear as g}
					<button class="chip" class:on={gearId === g.id} on:click={() => loadGear(g.id)}>{g.name}</button>
				{/each}
			</div>
			{#if gearLoading}
				<p style="font-size:13.5px;color:var(--td-muted);">Checking…</p>
			{:else if gearId && gearRows.length}
				<div style="margin-bottom:10px;">
					<PushPlaylist tracks={toTracks(gearRows)}
						title={`Never on ${data.gear.find((g) => g.id === gearId)?.name ?? 'this gear'}`}
						compact />
				</div>
				<div class="panel" style="border-radius:20px;overflow:hidden;">
					{#each gearRows as r, i}
						<button class="row" on:click={() => openRow(r)}>
							<CoverArt url={r.artwork_url} caaId={r.caa_id} releaseMbid={r.caa_release_mbid}
								seed={`${r.artist} ${r.name}`} size={38} radius={8} />
							<span style="flex:1;min-width:0;text-align:left;">
								<span class="nm">{r.name}</span>
								<span class="sb">{r.artist}{r.album ? ` — ${r.album}` : ''}</span>
							</span>
							<span class="meta">{r.plays} plays</span>
						</button>
						{#if i < gearRows.length - 1}<div class="sep"></div>{/if}
					{/each}
				</div>
			{:else if gearId}
				<p style="font-size:13.5px;color:var(--td-muted-2);">
					Everything you play often has been through this one.
				</p>
			{/if}
		{:else}
			<p style="font-size:13.5px;color:var(--td-muted-2);">
				No gear catalogued yet — add some under <a href="/chains">Signal Chains</a>.
			</p>
		{/if}
	</div>
{/if}

<style>
	.hero {
		background: var(--td-panel);
		color: #f4e7cf;
		border-radius: 28px;
		padding: 26px;
		margin-bottom: 26px;
		display: flex;
		gap: 26px;
		flex-wrap: wrap;
		align-items: center;
		box-shadow: 0 10px 30px rgba(46, 43, 37, 0.14);
	}
	/* 33⅓ — the same real speed the vinyl deck turns at. */
	.platter { animation: td-spin 1.8s linear infinite; }
	@media (prefers-reduced-motion: reduce) { .platter { animation: none; } }
	.eyebrow {
		font-size: 11px; letter-spacing: 0.16em; text-transform: uppercase;
		color: #c9a678; margin-bottom: 8px; font-weight: 600;
	}
	.pill {
		display: inline-flex; align-items: center; gap: 6px;
		font-size: 11.5px; font-weight: 600; color: #fff;
		border-radius: 999px; padding: 4px 11px; white-space: nowrap;
	}
	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
		gap: 18px;
	}
	.card {
		background: var(--td-surface, var(--td-panel));
		border: 1px solid var(--td-border, var(--td-line));
		border-radius: 22px;
		padding: 20px;
		display: flex;
		flex-direction: column;
	}
	/* An empty collection stays on the page rather than vanishing — that it is
	   empty is information, and a grid that reflows as data arrives is worse. */
	.card.empty { opacity: 0.62; }
	/* The open card keeps the accent ring its list is wearing, so the two read
	   as one thing even with the grid between them. */
	.card { transition: box-shadow 0.14s ease, border-color 0.14s ease; }
	@media (prefers-reduced-motion: reduce) { .card { transition: none; } }
	.badge {
		display: inline-flex; align-items: center; justify-content: center;
		width: 44px; height: 44px; flex: none; border-radius: 13px;
	}
	.tchip {
		display: inline-flex; align-items: center; max-width: 100%;
		background: var(--td-surface2, var(--td-panel2));
		border: 1px solid var(--td-border, var(--td-line));
		border-radius: 999px; padding: 5px 11px; font-size: 12px;
		color: var(--td-nav-fg, inherit);
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.expanded { border-radius: 20px; overflow: hidden; margin-top: 18px; border-width: 1px; border-style: solid; }
	.chip {
		border: 1px solid var(--td-line); background: transparent; color: var(--td-muted);
		border-radius: 999px; padding: 6px 14px; font-size: 12.5px; font-weight: 600; cursor: pointer;
	}
	.chip.on { background: var(--td-accent); border-color: var(--td-accent); color: #fff; }
	.linkish {
		border: none; background: none; padding: 0; font: inherit; font-size: 13px;
		color: var(--td-muted-2); cursor: pointer; text-decoration: underline; text-underline-offset: 3px;
	}
	.row {
		display: flex; align-items: center; gap: 12px; width: 100%;
		padding: 11px 16px; background: none; border: none; color: inherit;
		cursor: pointer; text-align: left;
	}
	.row:hover { background: var(--td-panel2); }
	.nm { display: block; font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.sb { display: block; font-size: 12.5px; color: var(--td-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.meta { font-size: 12.5px; color: var(--td-muted-2); flex: none; font-variant-numeric: tabular-nums; }
	.sep { height: 1px; background: var(--td-line); margin: 0 16px; }
	@media (max-width: 520px) { .meta { display: none; } }
</style>
