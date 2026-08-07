<script lang="ts">
	/**
	 * Loved — everything you've marked with a heart, in one place.
	 *
	 * Built from `temp/TapedeckNew.dc.html`, with one deliberate departure. The
	 * design calls this page "Loved songs" and filters it by fidelity
	 * (Lossless / DSD / Lossy). Neither transfers:
	 *
	 * * A love in Tapedeck is an **entity**, and entities come in three kinds —
	 *   a track, a record, or an artist. Calling the page "songs" and rendering
	 *   album and artist loves as if they were tracks would misdescribe two
	 *   thirds of what can land here, so the filter axis is *kind*.
	 * * Fidelity chips would hide almost everything. `is_lossless` is only known
	 *   for listens a live source reported — 2.6% of a real imported history —
	 *   so a "Lossless" filter reads as "you own almost nothing good". Same trap
	 *   the fidelity cards were just fixed for; see `FIDELITY_KNOWN` in `db.rs`.
	 *
	 * Everything else follows the design: search, the four-way sort with its
	 * grouped dividers, the stat strip, and the note snippet on the row.
	 *
	 * Filtering and sorting are client-side. Loves are inherently a small set —
	 * a few hundred at the very most — so one request beats a query parameter
	 * for every control on the page.
	 */
	import { onMount } from 'svelte';
	import { api, type LovedRow } from '$lib/api';
	import { openAlbum, openArtist } from '$lib/entities';
	import CoverArt from '$lib/components/CoverArt.svelte';
	import ToTop from '$lib/components/ToTop.svelte';

	type Sort = 'recent' | 'plays' | 'title' | 'artist';
	type Filter = 'all' | 'recording' | 'release' | 'artist' | 'notes';

	const SORTS: { k: Sort; label: string }[] = [
		{ k: 'recent', label: 'Recently loved' },
		{ k: 'plays', label: 'Most played' },
		{ k: 'title', label: 'Title' },
		{ k: 'artist', label: 'Artist' }
	];
	const FILTERS: { k: Filter; label: string }[] = [
		{ k: 'all', label: 'All' },
		{ k: 'recording', label: 'Tracks' },
		{ k: 'release', label: 'Albums' },
		{ k: 'artist', label: 'Artists' },
		{ k: 'notes', label: 'With notes' }
	];

	let rows: LovedRow[] = [];
	let loading = true;
	let error = '';
	let q = '';
	let sort: Sort = 'recent';
	let filter: Filter = 'all';
	/** Optimistic removals, so un-loving a row doesn't refetch the page. */
	let removed = new Set<number>();

	const nf = (n: number) => n.toLocaleString();

	async function load() {
		loading = true;
		try {
			rows = (await api.getLoves()).loves;
			error = '';
		} catch (e) {
			error = String(e).replace(/^API \d+: /, '');
		}
		loading = false;
	}

	async function unlove(r: LovedRow) {
		removed = new Set(removed).add(r.entity.id);
		try {
			await api.setLove({ entity_id: r.entity.id }, false);
		} catch {
			const back = new Set(removed);
			back.delete(r.entity.id);
			removed = back;
		}
	}

	/** A love opens the thing itself: a record for a track or an album, the
	 *  artist otherwise. Mirrors the "a track row always opens the record it's
	 *  on" rule the rest of the app follows. */
	function open(r: LovedRow) {
		if (r.entity.kind === 'artist') return openArtist(r.entity.name);
		if (r.entity.kind === 'release') return openAlbum(r.entity.artist_name, r.entity.name);
		if (r.album) return openAlbum(r.entity.artist_name, r.album);
		return openArtist(r.entity.artist_name);
	}

	const KIND_LABEL: Record<string, string> = {
		recording: 'Track',
		release: 'Album',
		artist: 'Artist'
	};

	function ago(ts: number): string {
		const d = Math.floor((Date.now() / 1000 - ts) / 86400);
		if (d < 1) return 'today';
		if (d < 7) return `${d}d`;
		if (d < 365) return `${Math.round(d / 30)}mo`;
		return `${Math.floor(d / 365)}y`;
	}

	/** The bucket a love falls in when sorted by when it happened. */
	function bucket(ts: number): string {
		const now = new Date();
		const then = new Date(ts * 1000);
		const days = (now.getTime() - then.getTime()) / 86400000;
		if (days < 7) return 'This week';
		if (days < 31) return 'This month';
		if (then.getFullYear() === now.getFullYear()) return `Earlier in ${then.getFullYear()}`;
		return String(then.getFullYear());
	}

	$: live = rows.filter((r) => !removed.has(r.entity.id));

	$: stats = (() => {
		const kinds = { recording: 0, release: 0, artist: 0 } as Record<string, number>;
		const artists: Record<string, number> = {};
		let withNotes = 0;
		for (const r of live) {
			kinds[r.entity.kind] = (kinds[r.entity.kind] ?? 0) + 1;
			const a = r.entity.kind === 'artist' ? r.entity.name : r.entity.artist_name;
			if (a) artists[a] = (artists[a] ?? 0) + 1;
			if (r.note_excerpt) withNotes += 1;
		}
		const top = Object.entries(artists).sort((a, b) => b[1] - a[1])[0];
		return [
			{ value: nf(live.length), label: live.length === 1 ? 'loved thing' : 'loved things' },
			{ value: `${nf(kinds.recording)} · ${nf(kinds.release)} · ${nf(kinds.artist)}`, label: 'tracks · albums · artists' },
			{ value: top ? top[0] : '—', label: 'most loved artist' },
			{ value: nf(withNotes), label: 'with liner notes' }
		];
	})();

	$: items = (() => {
		const needle = q.trim().toLowerCase();
		return live.filter((r) => {
			if (filter === 'notes' && !r.note_excerpt) return false;
			if (filter !== 'all' && filter !== 'notes' && r.entity.kind !== filter) return false;
			if (!needle) return true;
			return `${r.entity.name} ${r.entity.artist_name} ${r.album ?? ''}`
				.toLowerCase()
				.includes(needle);
		});
	})();

	/** Sorted *and* grouped — the two go together, since each sort implies the
	 *  divider it wants: a date bucket, a letter, an artist, or nothing. */
	$: groups = (() => {
		const out: { label: string; items: LovedRow[] }[] = [];
		const push = (label: string, r: LovedRow) => {
			const last = out[out.length - 1];
			if (last && last.label === label) last.items.push(r);
			else out.push({ label, items: [r] });
		};
		const list = [...items];
		if (sort === 'plays') {
			list.sort((a, b) => b.plays - a.plays);
			return [{ label: 'Most played', items: list }];
		}
		if (sort === 'title') {
			list.sort((a, b) => a.entity.name.localeCompare(b.entity.name));
			for (const r of list) push((r.entity.name.replace(/^["“'']/, '')[0] ?? '#').toUpperCase(), r);
			return out;
		}
		if (sort === 'artist') {
			const who = (r: LovedRow) => (r.entity.kind === 'artist' ? r.entity.name : r.entity.artist_name);
			list.sort((a, b) => who(a).localeCompare(who(b)) || b.plays - a.plays);
			for (const r of list) push(who(r) || '—', r);
			return out;
		}
		list.sort((a, b) => b.loved_at - a.loved_at);
		for (const r of list) push(bucket(r.loved_at), r);
		return out;
	})();

	onMount(load);
</script>

<svelte:head><title>Loved — Tapedeck</title></svelte:head>

<div class="page-head">
	<div style="min-width:0;">
		<div class="eyebrow">Your collection</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">Loved</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;">
			Tracks, records and artists you've marked with a heart — all of it from your
			own listening.
		</p>
	</div>
	<div class="search">
		<svg viewBox="0 0 24 24" width="17" fill="none" stroke="var(--td-muted-3)" stroke-width="2.4" stroke-linecap="round" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/></svg>
		<input class="input" bind:value={q} placeholder="Search title, artist, album…" aria-label="Search loved">
	</div>
</div>

{#if loading}
	<div style="display:flex;justify-content:center;padding:80px;">
		<div class="spin"></div>
	</div>
{:else if error}
	<div class="panel" style="border-radius:22px;padding:24px;">
		<p style="font-size:13.5px;color:var(--td-muted);margin:0;">{error}</p>
	</div>
{:else if !live.length}
	<div class="empty">
		Nothing loved yet. The heart on a listen, an album or an artist puts it here —
		and loves you already have on Last.fm or ListenBrainz can be pulled in from
		<a href="/settings#connections">Settings → Connections</a>.
	</div>
{:else}
	<div class="stats">
		{#each stats as s}
			<div class="stat">
				<div class="stat-v h-display">{s.value}</div>
				<div class="stat-k">{s.label}</div>
			</div>
		{/each}
	</div>

	<div class="controls">
		<div class="seg">
			{#each SORTS as s}
				<button class="seg-btn" class:on={sort === s.k} on:click={() => (sort = s.k)}>{s.label}</button>
			{/each}
		</div>
		<div class="chips">
			{#each FILTERS as f}
				<button class="chip" class:on={filter === f.k} on:click={() => (filter = f.k)}>{f.label}</button>
			{/each}
		</div>
	</div>

	{#if !items.length}
		<div class="empty">
			{q ? `Nothing loved matching “${q}”.` : 'Nothing here with this filter.'}
		</div>
	{/if}

	{#each groups as g (g.label)}
		<div class="divider">
			<span class="divider-l">{g.label}</span>
			<span class="rule"></span>
			<span class="divider-n">{g.items.length} {g.items.length === 1 ? 'item' : 'items'}</span>
		</div>
		<div class="panel list">
			{#each g.items as r, i (r.entity.id)}
				<div class="row" class:first={i === 0}>
					<div class="art" class:round={r.entity.kind === 'artist'}>
						<CoverArt
							url={r.artwork_url}
							caaId={r.caa_id}
							releaseMbid={r.caa_release_mbid}
							seed={`${r.entity.artist_name} ${r.entity.name}`}
							size={40}
							radius={r.entity.kind === 'artist' ? 999 : 9}
						/>
					</div>
					<button class="main" on:click={() => open(r)}>
						<span class="t">{r.entity.name}</span>
						<span class="a">
							{#if r.entity.kind === 'artist'}
								Artist
							{:else}
								{r.entity.artist_name}{r.album && r.album !== r.entity.name ? ` — ${r.album}` : ''}
							{/if}
						</span>
						{#if r.note_excerpt}
							<span class="note">
								<svg viewBox="0 0 24 24" width="13" fill="currentColor" aria-hidden="true"><path d="M7 7h5v5c0 2.5-1.7 4.3-4.3 5l-.5-1.4C8.8 15.1 10 14 10 12.5H7z"/><path d="M15 7h5v5c0 2.5-1.7 4.3-4.3 5l-.5-1.4c1.6-.5 2.8-1.6 2.8-3.1H15z"/></svg>
								<span class="note-t">{r.note_excerpt}</span>
							</span>
						{/if}
					</button>
					<span class="kind">{KIND_LABEL[r.entity.kind] ?? r.entity.kind}</span>
					<span class="plays">{r.plays ? `${nf(r.plays)} ${r.plays === 1 ? 'play' : 'plays'}` : '—'}</span>
					<span class="when" title={new Date(r.loved_at * 1000).toLocaleString()}>{ago(r.loved_at)}</span>
					<a
						class="act"
						class:has={r.note_id}
						href={`/notes?entity=${r.entity.id}`}
						title={r.note_id ? 'Edit liner notes' : 'Add liner notes'}
						aria-label="Liner notes"
					>
						<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 5h16"/><path d="M4 10h16"/><path d="M4 15h8"/><path d="M15.5 19.5l4.5-4.5-2-2-4.5 4.5-.6 2.6z"/></svg>
					</a>
					<button class="act unlove" on:click={() => unlove(r)} title="Remove from loved" aria-label="Remove from loved">
						<svg viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 20l-1.3-1.2C6 14.6 3 12 3 8.9 3 6.4 5 4.4 7.5 4.4c1.4 0 2.8.7 3.6 1.8"/><path d="M14 5.2a4.3 4.3 0 0 1 2.5-.8C19 4.4 21 6.4 21 8.9c0 1.6-.8 3.1-2.2 4.7"/><path d="M3 3l18 18"/></svg>
					</button>
				</div>
			{/each}
		</div>
	{/each}

	<ToTop />
{/if}

<style>
	.spin {
		width: 22px; height: 22px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		animation: td-spin 0.7s linear infinite;
	}
	.page-head {
		display: flex; align-items: flex-end; justify-content: space-between;
		gap: 16px; flex-wrap: wrap; margin-bottom: 20px;
	}
	.eyebrow {
		font-size: 11px; letter-spacing: 0.16em; text-transform: uppercase;
		color: var(--td-muted-2); margin-bottom: 6px;
	}
	.search { position: relative; width: 280px; max-width: 100%; }
	.search svg { position: absolute; left: 13px; top: 50%; transform: translateY(-50%); pointer-events: none; }
	.search .input { padding-left: 38px; }

	.stats {
		display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
		gap: 12px; margin-bottom: 20px;
	}
	.stat {
		background: var(--td-surface); border: 1px solid var(--td-border);
		border-radius: 16px; padding: 14px 16px; min-width: 0;
	}
	.stat-v {
		font-size: 23px; line-height: 1.05;
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.stat-k { font-size: 12px; color: var(--td-muted-2); margin-top: 5px; }

	.controls {
		display: flex; align-items: center; justify-content: space-between;
		gap: 12px; flex-wrap: wrap;
	}
	.seg {
		display: inline-flex; background: var(--td-sidebar);
		border-radius: 999px; padding: 4px; gap: 2px; flex-wrap: wrap;
	}
	.seg-btn {
		border: none; background: transparent; cursor: pointer; color: var(--td-nav-fg);
		font-size: 12.5px; padding: 6px 14px; border-radius: 999px; font-family: inherit;
	}
	.seg-btn.on { background: var(--td-accent); color: #fbf5ea; }
	.chips { display: flex; gap: 7px; flex-wrap: wrap; }
	.chip {
		border: 1px solid var(--td-line); background: transparent; color: var(--td-muted);
		border-radius: 999px; padding: 6px 13px; font-size: 12.5px; font-weight: 600;
		cursor: pointer; font-family: inherit;
	}
	.chip.on { background: var(--td-accent); border-color: var(--td-accent); color: #fff; }

	.divider { display: flex; align-items: center; gap: 12px; margin: 22px 0 10px; }
	.divider-l {
		font-size: 12px; letter-spacing: 0.12em; text-transform: uppercase;
		color: var(--td-muted-2); font-weight: 600;
	}
	.rule { flex: 1; height: 1px; background: var(--td-border); }
	.divider-n { font-size: 12px; color: var(--td-muted-3); }

	.list { border-radius: 22px; overflow: hidden; }
	.row {
		display: flex; align-items: center; gap: 13px;
		padding: 11px 18px; border-top: 1px solid var(--td-border);
	}
	.row.first { border-top: none; }
	.row:hover { background: var(--td-panel2); }
	.art { flex: none; }
	.main {
		flex: 1; min-width: 0; text-align: left; background: none;
		border: none; padding: 0; font: inherit; color: inherit; cursor: pointer;
	}
	.t {
		display: block; font-size: 14.5px; font-weight: 600;
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.a {
		display: block; font-size: 12.5px; color: var(--td-muted);
		white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
	}
	.main:hover .t { color: var(--td-accent); }
	.note {
		display: flex; align-items: center; gap: 6px; margin-top: 5px;
		font-size: 12px; color: var(--td-accent-2); min-width: 0;
	}
	.note svg { flex: none; }
	.note-t { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }

	.kind {
		font-size: 11px; color: var(--td-muted-3); flex: none;
		border: 1px solid var(--td-border); border-radius: 999px; padding: 2px 9px;
	}
	.plays {
		font-size: 12px; color: var(--td-muted-3); width: 72px;
		text-align: right; flex: none; font-variant-numeric: tabular-nums;
	}
	.when { font-size: 12px; color: var(--td-muted-3); width: 40px; text-align: right; flex: none; }

	.act {
		border: none; background: transparent; cursor: pointer; flex: none;
		display: flex; align-items: center; justify-content: center;
		width: 32px; height: 32px; border-radius: 9px; color: var(--td-muted-3);
	}
	.act:hover { background: var(--td-nav-hover); color: var(--td-text); }
	.act.has { background: color-mix(in srgb, var(--td-accent-2) 16%, transparent); color: var(--td-accent-2); }
	.unlove:hover { color: #b2622d; }

	.empty {
		background: var(--td-surface); border: 1px solid var(--td-border);
		border-radius: 22px; padding: 44px; text-align: center;
		color: var(--td-muted); margin-top: 16px; font-size: 14px; line-height: 1.6;
	}

	/* The row has seven fixed-width columns; below 720px the last three would
	   crush the title, which is the same mistake `ScrobbleRow` had to be rescued
	   from. The kind pill and the play count go — the title is what the row is
	   for, and the count is on the record's own page. */
	@media (max-width: 720px) {
		.kind, .plays { display: none; }
		.row { padding: 11px 14px; gap: 10px; }
	}
</style>
