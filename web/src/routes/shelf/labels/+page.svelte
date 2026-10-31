<script lang="ts">
	// Printable labels for the shelf, borrowed from DVinyl's QR labels and
	// pointed somewhere more useful: the code opens the record on *this* shelf,
	// with its sides ready to start. Stuck inside a sleeve or on a tape case, it
	// turns "find it in the app, open it, press start" into one scan — the
	// manual part the shelf was designed to make short.
	//
	// The code never starts a side by itself. A label can be scanned by
	// accident, and a listen is something the user says happened.
	//
	// The URL is built from `location.origin` in the browser rather than on
	// the server, because the browser is the one place that knows the address
	// this instance is actually reached at — behind a tunnel, a proxy or a
	// LAN name — which is the address the phone will scan its way to.
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { renderSVG } from 'uqr';
	import { api } from '$lib/api';
	import type { PhysicalRelease } from '$lib/api';
	import { syncUrl } from '$lib/url';

	let shelf: PhysicalRelease[] = [];
	let loading = true;
	let error = '';
	let picked = new Set<number>();
	let place = '';
	let origin = '';

	const KIND: Record<string, string> = { vinyl: 'Vinyl', cassette: 'Cassette', cd: 'CD', sacd: 'SACD' };

	$: places = [...new Set(shelf.map((r) => r.location?.trim()).filter((l): l is string => !!l))].sort(
		(a, b) => a.localeCompare(b)
	);
	$: listed = place ? shelf.filter((r) => (r.location ?? '').trim() === place) : shelf;
	$: chosen = shelf.filter((r) => picked.has(r.id));

	function remember() {
		const ids = [...picked].sort((a, b) => a - b).join(',');
		syncUrl(ids ? `/shelf/labels?ids=${ids}` : '/shelf/labels');
	}
	function toggle(id: number) {
		if (picked.has(id)) picked.delete(id);
		else picked.add(id);
		picked = picked;
		remember();
	}
	function pickListed(on: boolean) {
		for (const r of listed) {
			if (on) picked.add(r.id);
			else picked.delete(r.id);
		}
		picked = picked;
		remember();
	}

	/** Medium, then edition: "Vinyl · Clear". */
	const kindLine = (r: PhysicalRelease) => [KIND[r.kind] ?? r.kind, r.edition].filter(Boolean).join(' · ');

	// `M` error correction: a label lives inside a sleeve and gets scuffed, and
	// the URL is short enough that the extra redundancy costs nothing in size.
	const qr = (id: number) =>
		renderSVG(`${origin}/shelf?record=${id}`, { ecc: 'M', border: 1, pixelSize: 4 });

	onMount(async () => {
		origin = location.origin;
		const ids = ($page.url.searchParams.get('ids') ?? '')
			.split(',')
			.map((s) => Number(s))
			.filter((n) => Number.isInteger(n) && n > 0);
		picked = new Set(ids);
		try {
			shelf = (await api.listShelf()).releases;
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
	});
</script>

<svelte:head><title>Shelf labels — Tapedeck</title></svelte:head>

<div class="no-print">
	<p style="margin:0 0 6px;"><a href="/shelf" style="color:var(--td-muted);font-size:13px;">← The Shelf</a></p>
	<h1 class="h-display" style="font-size:30px;margin:0;">Labels</h1>
	<p style="margin:6px 0 18px;color:var(--td-muted);font-size:14px;max-width:640px;">
		A code for the inside of a sleeve or a tape case. Scanning it opens the record here with
		its sides ready to start — it never starts one on its own. Print on plain paper or label
		sheets; each label is 64 × 30 mm.
	</p>

	{#if loading}
		<p style="color:var(--td-muted);">Loading the shelf…</p>
	{:else if error}
		<p style="color:var(--td-bad-fg);">{error}</p>
	{:else if !shelf.length}
		<p style="color:var(--td-muted);">The shelf is empty — add a record first.</p>
	{:else}
		<div class="panel" style="border-radius:22px;padding:18px 20px;margin-bottom:18px;">
			<div style="display:flex;gap:10px;flex-wrap:wrap;align-items:center;margin-bottom:12px;">
				{#if places.length}
					<select class="input" style="max-width:260px;" bind:value={place} aria-label="Where it lives">
						<option value="">Everywhere</option>
						{#each places as p}<option value={p}>{p}</option>{/each}
					</select>
				{/if}
				<button class="btn btn-secondary" on:click={() => pickListed(true)}>Select these</button>
				<button class="btn btn-secondary" on:click={() => pickListed(false)}>Clear these</button>
				<span style="flex:1;"></span>
				<button class="btn btn-primary" disabled={!chosen.length} on:click={() => window.print()}>
					Print {chosen.length || ''} label{chosen.length === 1 ? '' : 's'}
				</button>
			</div>
			<div class="picklist">
				{#each listed as r (r.id)}
					<label class="pick">
						<input type="checkbox" checked={picked.has(r.id)} on:change={() => toggle(r.id)} />
						<span><strong>{r.artist}</strong> — {r.title}</span>
						<span style="color:var(--td-muted-2);">{kindLine(r)}</span>
					</label>
				{/each}
			</div>
		</div>
	{/if}
</div>

{#if chosen.length && origin}
	<div class="sheet">
		{#each chosen as r (r.id)}
			<div class="label">
				<div class="code">{@html qr(r.id)}</div>
				<div class="words">
					<div class="artist">{r.artist}</div>
					<div class="title">{r.title}</div>
					<div class="meta">{kindLine(r)}</div>
					{#if r.location}<div class="meta">{r.location}</div>{/if}
				</div>
			</div>
		{/each}
	</div>
{/if}

<style>
	.picklist {
		max-height: 340px;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.pick {
		display: flex;
		gap: 10px;
		align-items: baseline;
		flex-wrap: wrap;
		font-size: 13.5px;
		padding: 5px 4px;
		cursor: pointer;
	}
	.pick input {
		accent-color: var(--td-accent);
	}

	/* Labels are drawn at their printed size on screen too, so what you see is
	   what comes out. Black on white whatever the theme: it is going on paper. */
	.sheet {
		display: grid;
		grid-template-columns: repeat(auto-fill, 64mm);
		gap: 4mm;
	}
	.label {
		width: 64mm;
		height: 30mm;
		box-sizing: border-box;
		padding: 2.5mm;
		display: flex;
		gap: 2.5mm;
		align-items: center;
		background: white;
		color: black;
		border: 0.2mm dashed rgba(0, 0, 0, 0.35);
		border-radius: 2mm;
		overflow: hidden;
		break-inside: avoid;
	}
	.code {
		flex: none;
		width: 25mm;
		height: 25mm;
	}
	.code :global(svg) {
		width: 100%;
		height: 100%;
		display: block;
	}
	.words {
		min-width: 0;
		line-height: 1.2;
	}
	.artist {
		font-weight: 700;
		font-size: 9pt;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.title {
		font-size: 9pt;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.meta {
		font-size: 7pt;
		opacity: 0.7;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	/* Only the sheet reaches the paper. `visibility` rather than `display`, so
	   the app's own layout does not have to know this page exists. */
	@media print {
		:global(body *) {
			visibility: hidden;
		}
		.sheet,
		.sheet :global(*) {
			visibility: visible;
		}
		.sheet {
			position: absolute;
			left: 0;
			top: 0;
		}
		.label {
			border-color: rgba(0, 0, 0, 0.2);
		}
	}
</style>
