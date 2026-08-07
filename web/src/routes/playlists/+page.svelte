<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type PlaylistLab, type LabTrack } from '$lib/api';
	import { generate, buildExport, DEFAULTS, SOUND_DIMS, type Mode, type Params, type Result } from '$lib/playlistlab';
	import { openAlbum, openArtist } from '$lib/entities';
	import PushPlaylist from '$lib/components/PushPlaylist.svelte';

	/**
	 * Playlist Lab — grow a playlist by moving through the space your listening
	 * actually lives in.
	 *
	 * Built from `temp/Tapedeck-PlaylistLabs.dc.html`, with one substitution that
	 * has to stay visible. The design labels its ten axes Energy, Valence,
	 * Tempo, Acoustic, Danceable… and calls them "how they actually sound";
	 * Tapedeck has none of those numbers, and the mockup faked them by hashing
	 * the artist's name. The axes here are real — six from the Every Noise genre
	 * map, four from the user's own listening — and the page says so rather than
	 * borrowing the design's copy.
	 *
	 * The generators run here rather than on the server because the sliders drive
	 * them; see `$lib/playlistlab`.
	 */

	let data: PlaylistLab | null = null;
	let loading = true;
	let error = '';

	let mode: Mode = 'similar';
	let params: Params = { ...DEFAULTS };
	let seeds: number[] = [];

	let axisX = 0;
	let axisY = 1;

	let pickerOpen = false;
	let pickerQuery = '';
	let exportOpen = false;
	let exportName = '';
	let exportFmt: 'm3u' | 'json' | 'text' = 'm3u';
	let copied = false;

	const clean = (e: unknown) => String(e).replace(/^API \d+: /, '');

	const MODES: { key: Mode; title: string; blurb: string }[] = [
		{
			key: 'similar',
			title: 'Similar songs',
			blurb: 'One seed or more — we pull their nearest neighbours in the space.'
		},
		{
			key: 'crawl',
			title: 'Dimensional crawl',
			blurb: 'Two or more seeds — we trace a path between them, one step at a time.'
		},
		{
			key: 'coaster',
			title: 'Rollercoaster',
			blurb: 'A trajectory that climbs to peaks and dives to lows along one axis.'
		}
	];

	$: pool = data?.tracks ?? [];
	$: dims = data?.dims ?? [];
	$: result = pool.length ? generate(pool, seeds, mode, params) : ({ steps: [], centre: null, path: false, need: 1 } as Result);
	$: ready = result.steps.length > 0;

	// A seed list that outlives a mode change is the point — you pick two songs,
	// then try all three modes on them.
	$: needMsg =
		result.need === 0
			? ''
			: mode === 'crawl'
				? `A crawl needs two seeds to walk between — add ${result.need} more.`
				: 'Add a seed song to start.';

	function toggleSeed(i: number) {
		seeds = seeds.includes(i) ? seeds.filter((s) => s !== i) : [...seeds, i];
	}

	function defaultName(): string {
		const m = MODES.find((x) => x.key === mode)?.title ?? 'Playlist';
		const first = pool[seeds[0]];
		return first ? `${m} · ${first.artist}` : m;
	}

	function openExport() {
		exportName = defaultName();
		copied = false;
		exportOpen = true;
	}

	$: exportText = ready ? buildExport(result.steps, exportName || defaultName(), exportFmt) : '';

	async function copyExport() {
		try {
			await navigator.clipboard.writeText(exportText);
			copied = true;
			setTimeout(() => (copied = false), 2200);
		} catch {
			error = 'Could not reach the clipboard — use Download instead.';
		}
	}

	function downloadExport() {
		const ext = exportFmt === 'm3u' ? 'm3u8' : exportFmt === 'json' ? 'json' : 'txt';
		const blob = new Blob([exportText], { type: 'text/plain;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		a.href = url;
		a.download = `${(exportName || defaultName()).replace(/[^\w\- ]+/g, ' ').trim() || 'playlist'}.${ext}`;
		a.click();
		URL.revokeObjectURL(url);
	}

	/** What the "Send to…" button needs — names only, same as every other push. */
	$: pushTracks = result.steps.map((s) => ({
		title: s.track.title,
		artist: s.track.artist,
		album: s.track.album
	}));

	// ── The plot ──
	const PX = (v: number) => 11 + v * 110;
	const PY = (v: number) => 78 - v * 72;

	$: plotDots = pool
		// The backdrop is a sample, not the whole pool: three thousand dots is a
		// grey rectangle, and the shape is legible from a few hundred.
		.filter((_, i) => i % Math.max(1, Math.ceil(pool.length / 320)) === 0)
		.map((t) => ({ cx: PX(t.vector[axisX]), cy: PY(t.vector[axisY]) }));

	$: plotNodes = result.steps.map((s, i) => ({
		cx: PX(s.track.vector[axisX]),
		cy: PY(s.track.vector[axisY]),
		r: s.isSeed ? 3.2 : 2.6,
		fill: s.isSeed ? '#7a8a5e' : '#c67139',
		num: i + 1
	}));

	$: pathD = result.path && plotNodes.length > 1
		? plotNodes.map((n, i) => `${i ? 'L' : 'M'}${n.cx.toFixed(2)} ${n.cy.toFixed(2)}`).join(' ')
		: '';

	$: rings =
		mode === 'similar' && result.centre
			? [0.14, 0.26, 0.38].map((r) => ({
					cx: PX(result.centre![axisX]),
					cy: PY(result.centre![axisY]),
					r: r * 70
				}))
			: [];

	// ── Seed picker ──
	$: pickable = (() => {
		const q = pickerQuery.trim().toLowerCase();
		const lovedSet = new Set(data?.loved ?? []);
		const rows = pool
			.map((t, i) => ({ t, i, loved: lovedSet.has(i) }))
			.filter((r) => !q || `${r.t.title} ${r.t.artist} ${r.t.album ?? ''}`.toLowerCase().includes(q));
		// Loved first — the design's framing is that a playlist grows out of what
		// you have loved — but everything you have played is reachable, because a
		// song you forgot to love is exactly the one worth finding.
		rows.sort((a, b) => Number(b.loved) - Number(a.loved) || b.t.plays - a.t.plays);
		return rows.slice(0, 60);
	})();

	function openRow(t: LabTrack) {
		if (t.album) return openAlbum(t.artist, t.album);
		return openArtist(t.artist);
	}

	const initial = (s: string) => (s || '?').trim().charAt(0).toUpperCase();

	async function load(minPlays?: number) {
		loading = true;
		error = '';
		try {
			data = await api.playlistLab(minPlays);
			// Seed the seeds: the first loved track, so the page has something on
			// it the moment it opens rather than an empty plot and three sliders
			// that do nothing.
			if (!seeds.length && data.loved.length) seeds = [data.loved[0]];
			else if (!seeds.length && data.tracks.length) seeds = [0];
		} catch (e) {
			error = clean(e);
		}
		loading = false;
	}

	onMount(() => load());
</script>

<svelte:head><title>Playlist Lab — Tapedeck</title></svelte:head>

<div class="head">
	<div style="min-width:0;">
		<div class="eyebrow">Generative · {dims.length}-dimensional</div>
		<h1 class="h-display" style="font-size:38px;margin:0;">Playlist lab</h1>
		<p style="margin:6px 0 0;color:var(--td-muted);font-size:15px;max-width:56ch;">
			Grow a playlist by moving through the space your listening lives in — six axes
			from the genre map, four from how you actually play things.
		</p>
	</div>
	<div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
		<PushPlaylist tracks={pushTracks} title={defaultName()} />
		<button class="btn btn-primary" on:click={openExport} disabled={!ready}>
			<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor"
				stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
				<path d="M12 3v12" /><path d="M8 11l4 4 4-4" /><path d="M5 21h14" />
			</svg>
			Export
		</button>
	</div>
</div>

{#if error}
	<div class="panel warn">{error}</div>
{/if}

{#if loading}
	<p style="color:var(--td-muted);font-size:14px;">Placing your listening…</p>
{:else if data}
	<!-- What the space is. Stated on the page, not just in the API, because the
	     design's own copy promises something else and someone will read this and
	     assume Spotify-style audio features. -->
	<div class="basis">
		<strong>{data.pool.toLocaleString()} tracks</strong> in the pool, from
		{data.considered.toLocaleString()} you've played more than once.
		{#if data.unplaced}
			{data.unplaced.toLocaleString()} dropped — no genre of theirs has a place on the map.
		{/if}
		<span class="basis-note">{data.note}</span>
	</div>

	<div class="modes">
		{#each MODES as m}
			<button class="mode" class:on={mode === m.key} on:click={() => (mode = m.key)}>
				<span class="mode-top">
					<span class="mode-icon">
						{#if m.key === 'similar'}
							<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><circle cx="12" cy="12" r="9" /><circle cx="12" cy="12" r="5" /><circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" /></svg>
						{:else if m.key === 'crawl'}
							<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 17l5-8 4 5 5-9" /><circle cx="5" cy="17" r="1.7" fill="currentColor" stroke="none" /><circle cx="19" cy="5" r="1.7" fill="currentColor" stroke="none" /></svg>
						{:else}
							<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 15c2 0 2.5-8 5-8s2.5 10 5 10 3-9 5-9" /></svg>
						{/if}
					</span>
					<span class="h-display" style="font-size:16.5px;">{m.title}</span>
				</span>
				<span style="font-size:12.5px;color:var(--td-muted);line-height:1.45;">{m.blurb}</span>
			</button>
		{/each}
	</div>

	<div class="lab">
		<div class="col-controls">
			<div class="panel pad">
				<div class="panel-head">
					<h3 class="h-display" style="font-size:17px;margin:0;">Seed songs</h3>
					<span style="font-size:12px;color:var(--td-muted-3);">
						{seeds.length} chosen
					</span>
				</div>
				<p style="margin:0 0 14px;font-size:12.5px;color:var(--td-muted);">
					{mode === 'crawl' ? 'Two or more — the playlist walks from one to the next.' : 'Where the playlist starts from.'}
				</p>

				<div style="display:flex;flex-direction:column;gap:8px;margin-bottom:12px;">
					{#each seeds as si (si)}
						{@const t = pool[si]}
						{#if t}
							<div class="seed">
								<div class="cover">{initial(t.title)}</div>
								<div style="flex:1;min-width:0;">
									<div class="seed-t">{t.title}</div>
									<div class="seed-a">{t.artist}</div>
								</div>
								<button class="icon-btn" on:click={() => toggleSeed(si)} aria-label="Remove seed">
									<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
								</button>
							</div>
						{/if}
					{/each}
					{#if !seeds.length}
						<div class="empty-seed">No seeds yet — add a song to start.</div>
					{/if}
				</div>

				<button class="add-seed" on:click={() => { pickerOpen = true; pickerQuery = ''; }}>
					<svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M12 5v14M5 12h14" /></svg>
					Add seed song
				</button>

				{#if needMsg}
					<div class="need">
						<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><circle cx="12" cy="12" r="9" /><path d="M12 8v5M12 16.5v.01" /></svg>
						{needMsg}
					</div>
				{/if}
			</div>

			<div class="panel pad">
				<h3 class="h-display" style="font-size:17px;margin:0 0 16px;">Parameters</h3>

				<div class="slider">
					<div class="slider-head"><span>Playlist length</span><span>{params.length} songs</span></div>
					<input type="range" min="6" max="24" step="1" bind:value={params.length} />
				</div>

				{#if mode === 'similar'}
					<div class="slider">
						<div class="slider-head">
							<span>Diversity</span>
							<span>{params.diversity < 0.25 ? 'Tight' : params.diversity < 0.6 ? 'Balanced' : 'Adventurous'}</span>
						</div>
						<input type="range" min="0" max="1" step="0.05" bind:value={params.diversity} />
						<div class="ends"><span>Tightest matches</span><span>Adventurous</span></div>
					</div>
				{:else if mode === 'crawl'}
					<div class="slider">
						<div class="slider-head">
							<span>Step size</span>
							<span>{params.step < 0.25 ? 'Smooth' : params.step < 0.6 ? 'Even' : 'Bold'}</span>
						</div>
						<input type="range" min="0" max="1" step="0.05" bind:value={params.step} />
						<div class="ends"><span>Smooth, small hops</span><span>Bold leaps</span></div>
					</div>
				{:else}
					<div class="slider">
						<div class="slider-head">
							<span>Intensity</span>
							<span>{params.amp < 0.25 ? 'Gentle' : params.amp < 0.6 ? 'Rolling' : 'Wild'}</span>
						</div>
						<input type="range" min="0" max="1" step="0.05" bind:value={params.amp} />
						<div class="ends"><span>Gentle rolls</span><span>Wild swings</span></div>
					</div>
					<div class="field" style="margin-bottom:14px;">
						<label for="ride">Ride along</label>
						<select id="ride" class="input" bind:value={params.rideDim}>
							{#each dims as d, i}<option value={i}>{d.label}</option>{/each}
						</select>
					</div>
					<button class="btn btn-secondary" style="width:100%;gap:8px;font-size:13px;"
						on:click={() => (params = { ...params, seedN: params.seedN + 1 })}>
						<svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M3 5v5h5" /><path d="M4.2 14a8 8 0 1 0 1.3-6.3L3 10" /></svg>
						New trajectory
					</button>
				{/if}

				<!-- Not in the design, and it earns its place: the space mixes six
				     axes about the music with four about the listener, so without
				     this "similar" quietly drifts toward "played at the same hour". -->
				<div class="slider" style="margin-top:18px;padding-top:16px;border-top:1px solid var(--td-border);">
					<div class="slider-head">
						<span>Judge by</span>
						<span>{params.soundWeight >= 0.7 ? 'Mostly sound' : params.soundWeight <= 0.3 ? 'Mostly habits' : 'A bit of both'}</span>
					</div>
					<input type="range" min="0" max="1" step="0.05" bind:value={params.soundWeight} />
					<div class="ends"><span>How you listen</span><span>How it sounds</span></div>
				</div>
			</div>
		</div>

		<div class="panel pad col-plot">
			<div class="panel-head" style="align-items:flex-start;margin-bottom:14px;">
				<div>
					<h3 class="h-display" style="font-size:17px;margin:0 0 3px;">Phase space</h3>
					<p style="margin:0;font-size:12px;color:var(--td-muted);">
						A 2-D window onto the {dims.length}-D space. Pick the axes.
					</p>
				</div>
				<div style="display:flex;gap:8px;">
					<label class="axis">X axis
						<select class="input" bind:value={axisX}>
							{#each dims as d, i}<option value={i}>{d.label}</option>{/each}
						</select>
					</label>
					<label class="axis">Y axis
						<select class="input" bind:value={axisY}>
							{#each dims as d, i}<option value={i}>{d.label}</option>{/each}
						</select>
					</label>
				</div>
			</div>

			<svg viewBox="0 0 132 84" style="width:100%;height:auto;display:block;border-radius:14px;">
				<rect x="0.5" y="0.5" width="131" height="83" rx="8" fill="var(--td-surface2)" stroke="var(--td-border)" />
				{#each [44, 88] as gx}<line x1={gx} y1="6" x2={gx} y2="78" stroke="var(--td-border)" stroke-width="0.4" />{/each}
				{#each [30, 54] as gy}<line x1="11" y1={gy} x2="121" y2={gy} stroke="var(--td-border)" stroke-width="0.4" />{/each}

				{#each plotDots as p}<circle cx={p.cx} cy={p.cy} r="1.5" fill="rgba(122,112,96,.3)" />{/each}
				{#each rings as r}<circle cx={r.cx} cy={r.cy} r={r.r} fill="none" stroke="var(--td-accent)" stroke-width="0.5" stroke-dasharray="1.6 1.6" opacity="0.5" />{/each}
				{#if pathD}<path d={pathD} fill="none" stroke="var(--td-accent)" stroke-width="1" stroke-linejoin="round" stroke-linecap="round" opacity="0.8" />{/if}
				{#each plotNodes as n}
					<g>
						<circle cx={n.cx} cy={n.cy} r={n.r} fill={n.fill} stroke="var(--td-surface)" stroke-width="0.6" />
						<text x={n.cx} y={n.cy + 1.1} text-anchor="middle" font-size="3.1" font-weight="700" fill="#fbf5ea">{n.num}</text>
					</g>
				{/each}
			</svg>

			<div class="axis-ends">
				<span>→ {dims[axisX]?.low} … {dims[axisX]?.high}</span>
				<span>↑ {dims[axisY]?.low} … {dims[axisY]?.high}</span>
			</div>

			<div class="legend">
				<span><i style="background:#7a8a5e;border:1.5px solid var(--td-surface);"></i>Seed</span>
				<span><i style="background:#c67139;"></i>Playlist step</span>
				<span><i style="background:rgba(122,112,96,.3);"></i>Other tracks you've played</span>
			</div>
		</div>
	</div>

	<div class="head" style="margin:26px 0 12px;">
		<div style="display:flex;align-items:center;gap:12px;flex-wrap:wrap;">
			<h3 class="h-display" style="font-size:22px;margin:0;">Your playlist</h3>
			<span class="tag-accent">{MODES.find((m) => m.key === mode)?.title}</span>
		</div>
		<span style="font-size:13px;color:var(--td-muted);">
			{result.steps.length} song{result.steps.length === 1 ? '' : 's'}
		</span>
	</div>

	{#if ready}
		<div class="panel" style="border-radius:22px;overflow:hidden;">
			{#each result.steps as s, i (s.index)}
				<div class="row" class:seed-row={s.isSeed}>
					<span class="num">{i + 1}</span>
					<div class="cover">{initial(s.track.title)}</div>
					<button class="row-main" on:click={() => openRow(s.track)}>
						<span class="row-t">{s.track.title}</span>
						<span class="row-a">{s.track.artist}{s.track.album ? ` — ${s.track.album}` : ''}</span>
					</button>
					<span class="metric" class:is-seed={s.isSeed}>{s.metric}</span>
					<span class="plays">{s.track.plays} play{s.track.plays === 1 ? '' : 's'}</span>
				</div>
			{/each}
		</div>
	{:else}
		<div class="panel none">{needMsg || 'Nothing to build from yet.'}</div>
	{/if}
{/if}

{#if pickerOpen}
	<div class="dialog-backdrop" on:click={() => (pickerOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation" style="max-width:520px;">
			<div class="dialog-title">Add a seed song</div>
			<div class="dialog-body">
				<input class="input" bind:value={pickerQuery} placeholder="Search everything you've played" aria-label="Search tracks" />
				<p style="margin:8px 0 10px;font-size:12px;color:var(--td-muted-3);">
					Loved songs first, then everything else in the pool.
				</p>
				<div class="picklist">
					{#each pickable as r (r.i)}
						<button class="pick" class:on={seeds.includes(r.i)} on:click={() => toggleSeed(r.i)}>
							<span class="cover">{initial(r.t.title)}</span>
							<span style="flex:1;min-width:0;text-align:left;">
								<span class="seed-t">{r.t.title}</span>
								<span class="seed-a">{r.t.artist}</span>
							</span>
							{#if r.loved}<span style="color:#b2622d;font-size:13px;">♥</span>{/if}
							{#if seeds.includes(r.i)}<span style="color:var(--td-accent);font-size:13px;">✓</span>{/if}
						</button>
					{:else}
						<p style="font-size:13px;color:var(--td-muted);padding:12px;">Nothing matches that.</p>
					{/each}
				</div>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-primary" on:click={() => (pickerOpen = false)}>Done</button>
			</div>
		</div>
	</div>
{/if}

{#if exportOpen}
	<div class="dialog-backdrop" on:click={() => (exportOpen = false)} on:keydown={() => {}} role="presentation">
		<div class="dialog" on:click|stopPropagation on:keydown|stopPropagation role="presentation" style="max-width:560px;">
			<div class="dialog-title">Export playlist</div>
			<div class="dialog-body">
				<div class="field" style="margin-bottom:14px;">
					<label for="pl-name">Playlist name</label>
					<input id="pl-name" class="input" bind:value={exportName} />
				</div>
				<div style="display:flex;gap:6px;margin-bottom:12px;">
					{#each [['m3u', 'M3U'], ['json', 'JSON'], ['text', 'Text']] as [k, label]}
						<button class="fmt" class:on={exportFmt === k} on:click={() => (exportFmt = k as typeof exportFmt)}>{label}</button>
					{/each}
				</div>
				<div style="font-size:12.5px;color:var(--td-muted);margin-bottom:8px;">
					{result.steps.length} songs · {exportFmt === 'm3u' ? 'M3U8 file' : exportFmt === 'json' ? 'JSON file' : 'plain text'}
				</div>
				{#if exportFmt === 'm3u'}
					<!-- Said here rather than discovered when the playlist opens empty:
					     Tapedeck knows names, not where your files are. -->
					<p style="margin:0 0 10px;font-size:12px;color:var(--td-muted-3);line-height:1.5;">
						Names, not file paths — Tapedeck doesn't know where your library lives, so most
						players will list these without being able to open them. For a playlist that
						actually plays, close this and use <strong>Send to…</strong>, which resolves each
						track against the server it's going to.
					</p>
				{/if}
				<pre class="preview">{exportText.slice(0, 900)}</pre>
			</div>
			<div class="dialog-actions">
				<button class="btn btn-secondary" on:click={() => (exportOpen = false)}>Close</button>
				<button class="btn btn-secondary" on:click={copyExport}>{copied ? 'Copied' : 'Copy'}</button>
				<button class="btn btn-primary" on:click={downloadExport}>Download</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: 16px;
		flex-wrap: wrap;
		margin-bottom: 20px;
	}
	.eyebrow {
		font-size: 11px;
		letter-spacing: 0.16em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		margin-bottom: 6px;
	}
	.warn { border-radius: 16px; padding: 14px 18px; margin-bottom: 16px; font-size: 13.5px; color: #b2622d; }

	.basis {
		font-size: 12.5px;
		color: var(--td-muted);
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 10px 14px;
		margin-bottom: 18px;
		line-height: 1.5;
	}
	.basis-note { display: block; color: var(--td-muted-3); margin-top: 3px; }

	.modes { display: grid; grid-template-columns: repeat(auto-fit, minmax(215px, 1fr)); gap: 12px; margin-bottom: 20px; }
	.mode {
		display: flex;
		flex-direction: column;
		gap: 8px;
		text-align: left;
		padding: 16px;
		border-radius: 18px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		color: var(--td-text);
		font: inherit;
		cursor: pointer;
	}
	.mode:hover { border-color: var(--td-muted-3); }
	.mode.on { border-color: var(--td-accent); box-shadow: 0 0 0 3px rgba(198, 113, 57, 0.18); }
	.mode-top { display: flex; align-items: center; gap: 11px; }
	.mode-icon {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 34px;
		height: 34px;
		border-radius: 11px;
		background: rgba(198, 113, 57, 0.14);
		color: var(--td-accent);
		flex: none;
	}

	.lab { display: flex; gap: 16px; flex-wrap: wrap; align-items: flex-start; }
	.col-controls { flex: 1 1 300px; display: flex; flex-direction: column; gap: 14px; }
	.col-plot { flex: 1.7 1 360px; }
	.pad { border-radius: 20px; padding: 18px; }
	.panel-head { display: flex; align-items: baseline; justify-content: space-between; gap: 10px; margin-bottom: 4px; }

	.seed { display: flex; align-items: center; gap: 11px; padding: 8px 10px; background: var(--td-surface2); border: 1px solid var(--td-border); border-radius: 14px; }
	.seed-t { display: block; font-size: 13.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.seed-a { display: block; font-size: 12px; color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.empty-seed { padding: 14px; text-align: center; font-size: 12.5px; color: var(--td-muted-3); border: 1px dashed var(--td-border); border-radius: 14px; }

	.cover {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 34px;
		height: 34px;
		border-radius: 9px;
		background: var(--td-track);
		color: var(--td-muted);
		font-size: 13px;
		font-weight: 700;
		flex: none;
	}

	.icon-btn { border: none; background: transparent; cursor: pointer; color: var(--td-muted-3); padding: 5px; border-radius: 8px; flex: none; display: flex; }
	.icon-btn:hover { background: var(--td-nav-hover); }

	.add-seed {
		width: 100%;
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 8px;
		padding: 11px;
		border: 1.5px dashed var(--td-accent);
		background: transparent;
		color: var(--td-accent);
		border-radius: 999px;
		font: inherit;
		font-size: 13.5px;
		font-weight: 600;
		cursor: pointer;
	}
	.add-seed:hover { background: rgba(198, 113, 57, 0.09); }

	.need { margin-top: 11px; display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--td-accent); background: rgba(198, 113, 57, 0.1); border-radius: 12px; padding: 9px 12px; }

	.slider { margin-bottom: 18px; }
	.slider:last-child { margin-bottom: 0; }
	.slider-head { display: flex; justify-content: space-between; font-size: 12.5px; margin-bottom: 7px; }
	.slider-head span:first-child { font-weight: 600; }
	.slider-head span:last-child { color: var(--td-muted); }
	.slider input[type='range'] { width: 100%; accent-color: var(--td-accent); cursor: pointer; }
	.ends { display: flex; justify-content: space-between; font-size: 11px; color: var(--td-muted-3); margin-top: 4px; }

	.axis { display: flex; flex-direction: column; gap: 3px; font-size: 10.5px; letter-spacing: 0.08em; text-transform: uppercase; color: var(--td-muted-3); }
	.axis :global(select) { padding: 6px 8px; font-size: 12.5px; min-width: 110px; }

	.axis-ends { display: flex; align-items: center; justify-content: space-between; margin-top: 6px; font-size: 11px; color: var(--td-muted-3); gap: 10px; flex-wrap: wrap; }

	.legend { display: flex; flex-wrap: wrap; gap: 14px; margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--td-border); font-size: 11.5px; color: var(--td-muted); }
	.legend span { display: inline-flex; align-items: center; gap: 6px; }
	.legend i { width: 10px; height: 10px; border-radius: 50%; display: inline-block; }

	.tag-accent { background: rgba(198, 113, 57, 0.16); color: var(--td-accent); font-size: 12px; padding: 3px 10px; border-radius: 999px; }

	.row { display: flex; align-items: center; gap: 12px; padding: 10px 16px; border-bottom: 1px solid var(--td-border); }
	.row:last-child { border-bottom: none; }
	.seed-row { background: rgba(122, 138, 94, 0.08); }
	.num { width: 20px; text-align: right; font-size: 12px; color: var(--td-muted-3); flex: none; font-variant-numeric: tabular-nums; }
	.row-main { flex: 1; min-width: 0; text-align: left; border: none; background: none; font: inherit; color: inherit; cursor: pointer; padding: 0; }
	.row-t { display: block; font-size: 14.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.row-a { display: block; font-size: 12.5px; color: var(--td-muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
	.metric { font-size: 12px; color: var(--td-muted-3); flex: none; font-variant-numeric: tabular-nums; min-width: 44px; text-align: right; }
	.metric.is-seed { color: var(--td-good-fg); }
	.plays { font-size: 12px; color: var(--td-muted-3); width: 62px; text-align: right; flex: none; font-variant-numeric: tabular-nums; }

	.none { border-radius: 22px; padding: 44px; text-align: center; color: var(--td-muted); border-style: dashed; }

	.picklist { max-height: 340px; overflow-y: auto; display: flex; flex-direction: column; gap: 4px; }
	.pick { display: flex; align-items: center; gap: 10px; width: 100%; padding: 7px 10px; border: 1px solid transparent; border-radius: 12px; background: transparent; font: inherit; color: var(--td-text); cursor: pointer; }
	.pick:hover { background: var(--td-nav-hover); }
	.pick.on { border-color: var(--td-accent); background: var(--td-surface2); }

	.fmt { border: 1px solid var(--td-border); background: transparent; cursor: pointer; color: var(--td-muted-2); font: inherit; font-size: 12.5px; padding: 5px 14px; border-radius: 999px; }
	.fmt.on { background: var(--td-accent); border-color: var(--td-accent); color: #fbf5ea; }

	.preview { margin: 0; background: var(--td-panel); color: #e9dcc4; border-radius: 12px; padding: 14px; font-family: ui-monospace, monospace; font-size: 11.5px; line-height: 1.5; max-height: 170px; overflow: auto; white-space: pre-wrap; word-break: break-word; }

	@media (max-width: 720px) {
		.plays { display: none; }
	}
</style>
