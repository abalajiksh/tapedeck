<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type Rediscovery, type RediscoveryRow } from '$lib/api';
	import { formatDateTime } from '$lib/utils';
	import { openAlbum, openArtist } from '$lib/entities';
	import CoverArt from '$lib/components/CoverArt.svelte';

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

	async function setWindow(days: number) {
		quietDays = days;
		await load();
	}

	/** A track row opens the record it's on — the same rule every other track
	 *  listing in the app follows, falling back to the artist when there's no
	 *  album to open. */
	function openRow(r: RediscoveryRow) {
		if (r.album) return openAlbum(r.artist, r.album);
		return openArtist(r.artist);
	}

	function ago(ts: number | null): string {
		if (!ts) return 'never';
		const days = Math.floor((Date.now() / 1000 - ts) / 86400);
		if (days < 1) return 'today';
		if (days < 30) return `${days}d ago`;
		if (days < 365) return `${Math.floor(days / 30)}mo ago`;
		const y = Math.floor(days / 365);
		return `${y}y ago`;
	}

	onMount(load);
</script>

<svelte:head><title>Rediscover — Tapedeck</title></svelte:head>

<h1 class="h-display" style="font-size:34px;margin:0;">Rediscover</h1>
<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;max-width:60ch;">
	Nothing here is a recommendation — it's all things you already chose, and
	aren't choosing now.
</p>

<div style="display:flex;gap:8px;align-items:center;margin:18px 0;flex-wrap:wrap;">
	<span style="font-size:12.5px;color:var(--td-muted-2);">Quiet for</span>
	{#each WINDOWS as w}
		<button class="chip" class:on={quietDays === w.days} on:click={() => setWindow(w.days)}>
			{w.label}
		</button>
	{/each}
</div>

{#if error}
	<p style="color:#b2622d;font-size:14px;">{error}</p>
{/if}

{#if loading}
	<p style="color:var(--td-muted);font-size:14px;">Looking through your history…</p>
{:else if data}
	{#snippet list(title: string, blurb: string, rows: RediscoveryRow[])}
		<section style="margin-bottom:28px;">
			<h3 class="h-display" style="font-size:20px;margin:0 0 2px;">{title}</h3>
			<p style="margin:0 0 12px;font-size:13px;color:var(--td-muted);">{blurb}</p>
			{#if rows.length}
				<div class="panel" style="border-radius:20px;overflow:hidden;">
					{#each rows as r, i}
						<button class="row" on:click={() => openRow(r)}>
							<CoverArt url={r.artwork_url} caaId={r.caa_id} releaseMbid={r.caa_release_mbid}
								seed={`${r.artist} ${r.name}`} size={38} radius={8} />
							<span style="flex:1;min-width:0;text-align:left;">
								<span class="nm">{r.name}</span>
								<span class="sb">{r.artist}{r.album ? ` — ${r.album}` : ''}</span>
							</span>
							<span class="meta">
								{r.plays} play{r.plays === 1 ? '' : 's'}
								<span style="color:var(--td-muted-3);">· {ago(r.last_played)}</span>
							</span>
						</button>
						{#if i < rows.length - 1}<div class="sep"></div>{/if}
					{/each}
				</div>
			{:else}
				<p style="font-size:13.5px;color:var(--td-muted-2);margin:0;">
					Nothing here yet — which is its own kind of answer.
				</p>
			{/if}
		</section>
	{/snippet}

	{@render list(
		'Forgotten favourites',
		`Played properly once, and not in the last ${WINDOWS.find((w) => w.days === quietDays)?.label ?? 'while'}.`,
		data.forgotten_favourites
	)}

	{@render list(
		'Loved, barely played',
		'You went out of your way to love these, then didn’t come back.',
		data.loved_but_unplayed
	)}

	{@render list(
		'Heard once',
		'One listen, long enough ago that you’d have forgotten whether it landed.',
		data.heard_once
	)}

	{@render list(
		'This time of year',
		'What you were playing around now, in years gone by.',
		data.seasonal
	)}

	<section style="margin-bottom:28px;">
		<h3 class="h-display" style="font-size:20px;margin:0 0 2px;">Never heard on…</h3>
		<p style="margin:0 0 12px;font-size:13px;color:var(--td-muted);">
			Favourites that have never been through a particular piece of gear. Needs
			listens with a signal chain attached — that's where the connection comes
			from.
		</p>

		{#if data.gear.length}
			<div style="display:flex;gap:8px;flex-wrap:wrap;margin-bottom:12px;">
				{#each data.gear as g}
					<button class="chip" class:on={gearId === g.id} on:click={() => loadGear(g.id)}>
						{g.name}
					</button>
				{/each}
			</div>

			{#if gearLoading}
				<p style="font-size:13.5px;color:var(--td-muted);">Checking…</p>
			{:else if gearId && gearRows.length}
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
	</section>
{/if}

<style>
	.chip {
		border: 1px solid var(--td-line);
		background: transparent;
		color: var(--td-muted);
		border-radius: 999px;
		padding: 6px 14px;
		font-size: 12.5px;
		font-weight: 600;
		cursor: pointer;
	}
	.chip.on { background: var(--td-accent); border-color: var(--td-accent); color: #fff; }
	.row {
		display: flex;
		align-items: center;
		gap: 12px;
		width: 100%;
		padding: 11px 16px;
		background: none;
		border: none;
		color: inherit;
		cursor: pointer;
		text-align: left;
	}
	.row:hover { background: var(--td-panel2); }
	.nm { display: block; font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.sb { display: block; font-size: 12.5px; color: var(--td-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.meta { font-size: 12.5px; color: var(--td-muted-2); flex: none; font-variant-numeric: tabular-nums; }
	.sep { height: 1px; background: var(--td-line); margin: 0 16px; }
	/* The lists stack on a phone rather than squeezing the metadata off the
	   right-hand edge, which is what killed the track name on the history row. */
	@media (max-width: 520px) {
		.meta { display: none; }
	}
</style>
