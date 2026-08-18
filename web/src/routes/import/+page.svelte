<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { api, type ImportJob, type ImportRecord } from '$lib/api';
	import { formatDateTime } from '$lib/utils';

	type Src = 'listenbrainz' | 'lastfm' | 'file';
	let src: Src = 'listenbrainz';

	let lbUser = '', lbToken = '', fmUser = '', file: File | null = null;

	// Rockbox writes `.scrobbler.log` with a `#TZ` header. When it says UNKNOWN
	// the timestamps came off a device clock with no idea what zone it was in, so
	// we have to ask — guessing shifts a whole player's history permanently.
	let logNeedsTz = false;
	let deviceTz = typeof Intl !== 'undefined'
		? Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
		: 'UTC';
	const zones: string[] = (() => {
		try { return (Intl as any).supportedValuesOf?.('timeZone') ?? []; }
		catch { return []; }
	})();

	// ── Adding a listen by hand ──
	//
	// Goes through /1/submit-listens, the same endpoint external clients use, so
	// a hand-entered listen gets the same validation, dedup and chain resolution
	// as a real one. Parsed and shown back before anything is sent — the whole
	// point of typing one out is that you meant something specific by it.
	let manualJson = '';
	let manualBusy = false;
	let manualMsg = '';
	let manualErr = '';

	type ParsedListen = { title: string; artist: string; album?: string; at?: number };

	/** Accepts a bare listen, an array of them, or a full submit-listens body. */
	function extractListens(raw: string): unknown[] {
		const v = JSON.parse(raw);
		if (Array.isArray(v)) return v;
		if (v && typeof v === 'object') {
			const o = v as Record<string, unknown>;
			if (Array.isArray(o.payload)) return o.payload;
			if (Array.isArray(o.listens)) return o.listens;
			return [v];
		}
		throw new Error('Expected a JSON object or array');
	}

	function describe(l: unknown): ParsedListen {
		const o = (l ?? {}) as Record<string, unknown>;
		const tm = (o.track_metadata ?? o) as Record<string, unknown>;
		const str = (x: unknown) => (typeof x === 'string' && x.trim() ? x.trim() : undefined);
		const title = str(tm.track_name) ?? str(tm.title);
		const artist = str(tm.artist_name) ?? str(tm.artist);
		if (!title || !artist) throw new Error('Each listen needs a track name and an artist');
		const at = typeof o.listened_at === 'number' ? o.listened_at : undefined;
		return { title, artist, album: str(tm.release_name) ?? str(tm.album), at };
	}

	// Reactive so the preview tracks what is typed; an invalid draft is a normal
	// mid-typing state, not an error to shout about.
	$: manualParsed = (() => {
		if (!manualJson.trim()) return { ok: true as const, listens: [] as ParsedListen[] };
		try {
			return { ok: true as const, listens: extractListens(manualJson).map(describe) };
		} catch (e) {
			return { ok: false as const, error: (e as Error).message };
		}
	})();

	async function submitManual() {
		if (!manualParsed.ok || !manualParsed.listens.length) return;
		manualBusy = true; manualMsg = ''; manualErr = '';
		try {
			const listens = extractListens(manualJson);
			// `single` is for something playing now and carries no timestamp
			// requirement; anything with a listened_at is a past listen, which
			// is what `import` means to ListenBrainz.
			const anyTimestamped = manualParsed.listens.some((l) => l.at);
			await api.submitListens(listens, anyTimestamped ? 'import' : 'single');
			manualMsg = `Added ${listens.length} listen${listens.length === 1 ? '' : 's'}.`;
			manualJson = '';
			await loadHistory();
		} catch (e) {
			manualErr = String(e).replace(/^API \d+: /, '');
		}
		manualBusy = false;
	}

	async function inspectFile(f: File | null) {
		file = f;
		logNeedsTz = false;
		if (!f) return;
		// Only the header matters, so read the first slice rather than the file.
		// A zip starts with PK\x03\x04 and is never a Rockbox log, so skip the
		// sniffing entirely rather than reading binary as text.
		if (f.name.toLowerCase().endsWith('.zip')) return;
		const head = await f.slice(0, 512).text();
		const isLog = head.split('\n').slice(0, 5).some((l) => l.trim().toUpperCase().startsWith('#AUDIOSCROBBLER'));
		if (!isLog) return;
		const tzLine = head.split('\n').find((l) => l.trim().startsWith('#TZ/'));
		logNeedsTz = !tzLine || !tzLine.trim().slice(4).trim().toUpperCase().startsWith('UTC');
	}
	let job: ImportJob | null = null;
	let history: ImportRecord[] = [];
	let error = '';
	let poll: ReturnType<typeof setInterval> | null = null;

	const tabs: { v: Src; title: string; sub: string }[] = [
		{ v: 'listenbrainz', title: 'ListenBrainz', sub: 'Pull via username + token' },
		{ v: 'lastfm', title: 'Last.fm', sub: 'Pull scrobbles by username' },
		{ v: 'file', title: 'Upload Export', sub: 'ListenBrainz, Spotify, CSV, Rockbox' }
	];

	$: running = job?.state === 'running';
	$: done = job?.state === 'done';
	$: pct = job && job.total ? Math.min(100, Math.round((job.fetched / job.total) * 100)) : null;

	function startPolling(jobId: string) {
		stopPolling();
		poll = setInterval(async () => {
			try {
				job = await api.importJob(jobId);
				if (job.state !== 'running') { stopPolling(); loadHistory(); }
			} catch { stopPolling(); }
		}, 1000);
	}
	function stopPolling() { if (poll) { clearInterval(poll); poll = null; } }

	async function start() {
		error = '';
		try {
			let res: { job_id: string };
			if (src === 'listenbrainz') {
				if (!lbUser) return (error = 'Enter a ListenBrainz username');
				res = await api.importListenBrainz(lbUser, lbToken || undefined);
			} else if (src === 'lastfm') {
				if (!fmUser) return (error = 'Enter a Last.fm username');
				res = await api.importLastfm(fmUser);
			} else {
				if (!file) return (error = 'Choose a file to upload');
				res = await api.importFile(file, logNeedsTz ? deviceTz : undefined);
			}
			job = await api.importJob(res.job_id);
			startPolling(res.job_id);
		} catch (e) {
			error = e instanceof Error ? e.message.replace(/^API \d+: /, '') : String(e);
		}
	}

	function reset() { job = null; error = ''; }

	async function loadHistory() {
		try { history = (await api.importHistory()).imports; } catch { /* optional */ }
	}

	// An import keeps running server-side when you leave the page, so on arrival
	// ask whether one of ours is still going and re-attach to it. Without this
	// the progress bar simply vanished and looked like the import had died.
	async function resumeRunningJob() {
		try {
			const { jobs } = await api.activeImportJobs();
			if (jobs.length) {
				job = jobs[0];
				startPolling(jobs[0].job_id);
			}
		} catch { /* nothing running, or not reachable — no bar either way */ }
	}

	onMount(() => {
		loadHistory();
		resumeRunningJob();
	});
	onDestroy(stopPolling);
</script>

<svelte:head><title>Import — Tapedeck</title></svelte:head>

<h1 class="h-display" style="font-size:38px;margin-bottom:6px;">Import</h1>
<p style="margin:0 0 24px;color:var(--td-muted);font-size:15px;">Bring your whole listening history home — pull from a service or upload an export.</p>

<div style="display:flex;gap:12px;flex-wrap:wrap;margin-bottom:20px;">
	{#each tabs as tab}
		<button class="src-tab" class:on={src === tab.v} on:click={() => (src = tab.v)}>
			<div style="font-weight:700;font-size:15px;">{tab.title}</div>
			<div style="font-size:12.5px;opacity:.75;">{tab.sub}</div>
		</button>
	{/each}
</div>

<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:20px;">
	{#if src === 'listenbrainz'}
		<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Pull from ListenBrainz</h3>
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 18px;">We'll page through your listens via the ListenBrainz API. The token is only needed for private profiles.</p>
		<div style="display:flex;flex-direction:column;gap:14px;max-width:440px;">
			<div class="field"><label for="lu">ListenBrainz username</label><input id="lu" class="input" bind:value={lbUser} placeholder="your-lb-name"></div>
			<div class="field"><label for="lt">User token <span style="opacity:.6">(optional)</span></label><input id="lt" class="input" type="password" bind:value={lbToken} placeholder="listenbrainz.org → Settings"></div>
			<button class="btn btn-primary" style="align-self:flex-start;" on:click={start} disabled={running}>Start import</button>
		</div>
	{:else if src === 'lastfm'}
		<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Pull from Last.fm</h3>
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 18px;">Last.fm exposes public scrobbles by username — no token required. Private profiles must upload an export instead.</p>
		<div style="display:flex;flex-direction:column;gap:14px;max-width:440px;">
			<div class="field"><label for="fu">Last.fm username</label><input id="fu" class="input" bind:value={fmUser} placeholder="your-fm-name"></div>
			<button class="btn btn-primary" style="align-self:flex-start;" on:click={start} disabled={running}>Start import</button>
		</div>
	{:else}
		<h3 class="h-display" style="font-size:19px;margin-bottom:4px;">Upload an export</h3>
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 18px;">
			Drop the <code class="chip">.zip</code> straight from ListenBrainz's “Export your data”
			— listens <em>and</em> loved tracks come across, with the MusicBrainz IDs already in it, so
			nothing has to be looked up again. Spotify's <strong>Extended Streaming History</strong>
			zip works too, and is read as what it is: play lengths recovered from the history itself,
			anything you barely played filed as a skip, podcasts and audiobooks left out.
			A single <code class="chip">.json</code>/<code class="chip">.jsonl</code>,
			a <code class="chip">.csv</code> export, or a Rockbox <code class="chip">.scrobbler.log</code>
			off your DAP all work too.
		</p>
		<p style="font-size:12.5px;color:var(--td-muted-2);margin:-8px 0 18px;">
			A Spotify export carries no MusicBrainz IDs, so those listens land first and fill in
			their metadata and artwork slowly afterwards — a large history takes a while to finish
			enriching. Ask Spotify for it under Privacy Settings → “Extended streaming history”;
			it arrives by email, and the smaller “Account data” download is <em>not</em> the same file.
		</p>
		<label style="display:flex;flex-direction:column;align-items:center;justify-content:center;gap:10px;border:2px dashed rgba(198,113,57,.4);border-radius:18px;padding:34px;cursor:pointer;background:var(--td-surface2);max-width:440px;">
			<svg viewBox="0 0 24 24" width="30" fill="none" stroke="#c67139" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 16V4"/><path d="M8 8l4-4 4 4"/><path d="M4 20h16"/></svg>
			<span style="font-size:14px;color:var(--td-nav-fg);font-weight:600;">{file ? file.name : 'Choose a file'}</span>
			<span style="font-size:12px;color:var(--td-muted-2);">.zip, .json, .jsonl, .csv or .log</span>
			<input type="file" accept=".zip,.json,.jsonl,.csv,.log,.txt" on:change={(e) => inspectFile(e.currentTarget.files?.[0] ?? null)} style="display:none;">
		</label>

		{#if logNeedsTz}
			<div style="margin-top:16px;max-width:440px;">
				<div class="field">
					<label for="dev-tz">Player's timezone</label>
					<input id="dev-tz" class="input" list="tz-zones" bind:value={deviceTz} placeholder="Europe/Berlin">
					<datalist id="tz-zones">{#each zones as z}<option value={z}></option>{/each}</datalist>
				</div>
				<p style="font-size:12.5px;color:var(--td-muted-2);margin:8px 0 0;">
					This log says <code class="chip">#TZ/UNKNOWN</code> — its timestamps came from the
					player's own clock. Pick the zone the device was set to, or every listen lands at the
					wrong hour and the listening clock is skewed for good.
				</p>
			</div>
		{/if}
		<button class="btn btn-primary" style="margin-top:16px;" on:click={start} disabled={running}>Import file</button>
	{/if}

	{#if error}
		<div style="margin-top:16px;font-size:13px;color:#b2622d;">{error}</div>
	{/if}
</div>

{#if running}
	<div style="background:var(--td-panel);color:#f4e7cf;border-radius:22px;padding:24px;margin-bottom:20px;">
		<div style="display:flex;align-items:center;gap:10px;margin-bottom:14px;">
			<span style="width:8px;height:8px;border-radius:50%;background:#e07a55;animation:td-blink 1.4s ease-in-out infinite;"></span>
			<span style="font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:#c9a678;">Importing from {job?.source}</span>
		</div>
		<div style="height:12px;border-radius:999px;background:var(--td-panel2);overflow:hidden;margin-bottom:10px;">
			<div style="width:{pct ?? 30}%;height:100%;background:linear-gradient(90deg,#c67139,#e07a55);border-radius:999px;transition:width .3s;{pct === null ? 'animation:td-blink 1.2s ease-in-out infinite;' : ''}"></div>
		</div>
		<div style="display:flex;justify-content:space-between;font-size:13px;color:#b3a487;font-variant-numeric:tabular-nums;">
			<span>{job?.fetched ?? 0}{job?.total ? ` / ${job.total}` : ''} listens</span><span>{pct !== null ? pct + '%' : 'working…'}</span>
		</div>
	</div>
{:else if done}
	<div style="background:var(--td-good-bg);border:1px solid rgba(122,138,94,.3);border-radius:22px;padding:22px;margin-bottom:20px;display:flex;align-items:center;justify-content:space-between;gap:16px;flex-wrap:wrap;">
		<div style="display:flex;align-items:center;gap:12px;">
			<span style="display:inline-flex;align-items:center;justify-content:center;width:38px;height:38px;border-radius:50%;background:#7a8a5e;color:#fff;">
				<svg viewBox="0 0 24 24" width="20" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M5 13l4 4L19 7"/></svg></span>
			<div>
				<div style="font-weight:600;font-size:15px;color:var(--td-good-fg);">Imported {job?.imported} new listens from {job?.source}</div>
				<!-- A re-import of a history you already hold correctly inserts
				     nothing, and "0 new listens" alone read as though the whole
				     job had been pointless when it had in fact filled in a great
				     deal. Silent when there was nothing to fill. -->
				{#if job?.enriched}
					<div style="font-size:12.5px;color:var(--td-good-fg);margin-top:2px;">
						Filled in missing details on {job.enriched} listen{job.enriched === 1 ? '' : 's'} you
						already had — album, length, MusicBrainz ids, cover art. Nothing already there was changed.
					</div>
				{/if}
				<div style="font-size:12.5px;color:var(--td-good-fg);margin-top:2px;">Deduplicated against your existing history ({job?.fetched} scanned).</div>
			</div>
		</div>
		<div style="display:flex;gap:8px;">
			<button class="btn btn-secondary" on:click={reset}>Import more</button>
			<a class="btn btn-primary" href="/history" style="text-decoration:none;">View history</a>
		</div>
	</div>
{/if}

<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Add a listen by hand</h3>
<div class="panel" style="border-radius:22px;padding:24px;margin-bottom:20px;">
	<p style="font-size:13px;color:var(--td-muted);margin:0 0 14px;">
		Paste one ListenBrainz-shaped listen — or an array of them, or a whole
		<code>submit-listens</code> body. It goes through the same endpoint your
		scrobble clients use, so it is deduplicated and gets a signal chain like
		any other listen. <code>listened_at</code> is unix seconds; leave it out
		and the listen is treated as playing now.
	</p>

	<textarea
		class="input"
		rows="8"
		spellcheck="false"
		bind:value={manualJson}
		placeholder={'{\n  "listened_at": 1785858587,\n  "track_metadata": {\n    "artist_name": "Chris Isaak",\n    "track_name": "Wicked Game",\n    "release_name": "Best of Chris Isaak"\n  }\n}'}
		style="font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12.5px;line-height:1.55;resize:vertical;width:100%;"
	></textarea>

	{#if manualJson.trim()}
		<div style="margin-top:12px;">
			{#if !manualParsed.ok}
				<div style="font-size:13px;color:#b2622d;">✗ {manualParsed.error}</div>
			{:else}
				<div style="font-size:12px;letter-spacing:.12em;text-transform:uppercase;color:var(--td-muted-2);margin-bottom:8px;">
					{manualParsed.listens.length} listen{manualParsed.listens.length === 1 ? '' : 's'} ready
				</div>
				{#each manualParsed.listens.slice(0, 5) as l}
					<div style="display:flex;gap:10px;align-items:baseline;font-size:13.5px;padding:5px 0;border-top:1px solid var(--td-border);">
						<span style="font-weight:600;">{l.title}</span>
						<span style="color:var(--td-muted);flex:1;min-width:0;">
							{l.artist}{l.album ? ` — ${l.album}` : ''}
						</span>
						<span style="color:var(--td-muted-2);font-variant-numeric:tabular-nums;">
							{l.at ? formatDateTime(l.at) : 'now'}
						</span>
					</div>
				{/each}
				{#if manualParsed.listens.length > 5}
					<div style="font-size:12.5px;color:var(--td-muted-2);padding-top:6px;">
						…and {manualParsed.listens.length - 5} more.
					</div>
				{/if}
			{/if}
		</div>
	{/if}

	<div style="display:flex;align-items:center;gap:12px;margin-top:16px;flex-wrap:wrap;">
		<button
			class="btn btn-primary"
			on:click={submitManual}
			disabled={manualBusy || !manualParsed.ok || !manualParsed.listens?.length}
		>{manualBusy ? 'Adding…' : 'Add listen'}</button>
		{#if manualMsg}<span style="font-size:13px;color:var(--td-good-fg);">{manualMsg}</span>{/if}
		{#if manualErr}<span style="font-size:13px;color:#b2622d;">{manualErr}</span>{/if}
	</div>
</div>

<h3 class="h-display" style="font-size:19px;margin:0 0 12px;">Past Imports</h3>
<div class="panel" style="border-radius:22px;overflow:hidden;padding:6px 20px 14px;">
	{#if history.length}
		<table class="table">
			<thead><tr><th>Source</th><th>Date</th><th style="text-align:right;">Listens</th><th style="text-align:right;">Status</th></tr></thead>
			<tbody>
				{#each history as h}
					<tr>
						<td style="font-weight:600;text-transform:capitalize;">{h.source}</td>
						<td style="color:var(--td-muted);">{formatDateTime(h.date)}</td>
						<td style="text-align:right;color:var(--td-muted);font-variant-numeric:tabular-nums;">{h.count}</td>
						<td style="text-align:right;"><span class="tag {h.status === 'done' ? 'tag-accent-2' : h.status === 'error' ? 'tag-outline' : 'tag-neutral'}">{h.status}</span></td>
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p style="color:var(--td-muted);font-size:14px;padding:16px 0;margin:0;">No imports yet.</p>
	{/if}
</div>

<style>
	.src-tab {
		flex: 1; min-width: 180px; text-align: left; cursor: pointer;
		background: var(--td-surface); border: 1px solid var(--td-border); border-radius: 18px;
		padding: 16px 18px; color: var(--td-text);
	}
	.src-tab.on { border-color: var(--td-accent); background: var(--td-surface2); box-shadow: 0 0 0 1px var(--td-accent) inset; }
	.chip { background: var(--td-sidebar); padding: 2px 6px; border-radius: 6px; font-size: 12px; }
</style>
