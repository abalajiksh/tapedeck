<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { api, type ImportJob, type ImportRecord } from '$lib/api';
	import { formatDateTime } from '$lib/utils';

	type Src = 'listenbrainz' | 'lastfm' | 'file';
	let src: Src = 'listenbrainz';

	let lbUser = '', lbToken = '', fmUser = '', file: File | null = null;
	let job: ImportJob | null = null;
	let history: ImportRecord[] = [];
	let error = '';
	let poll: ReturnType<typeof setInterval> | null = null;

	const tabs: { v: Src; title: string; sub: string }[] = [
		{ v: 'listenbrainz', title: 'ListenBrainz', sub: 'Pull via username + token' },
		{ v: 'lastfm', title: 'Last.fm', sub: 'Pull scrobbles by username' },
		{ v: 'file', title: 'Upload Export', sub: '.json / .csv from either' }
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
				res = await api.importFile(file);
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

	onMount(loadHistory);
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
		<p style="font-size:13px;color:var(--td-muted);margin:0 0 18px;">Drop a ListenBrainz <code class="chip">.json</code> dump or a <code class="chip">.csv</code> export.</p>
		<label style="display:flex;flex-direction:column;align-items:center;justify-content:center;gap:10px;border:2px dashed rgba(198,113,57,.4);border-radius:18px;padding:34px;cursor:pointer;background:var(--td-surface2);max-width:440px;">
			<svg viewBox="0 0 24 24" width="30" fill="none" stroke="#c67139" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 16V4"/><path d="M8 8l4-4 4 4"/><path d="M4 20h16"/></svg>
			<span style="font-size:14px;color:var(--td-nav-fg);font-weight:600;">{file ? file.name : 'Choose a file'}</span>
			<span style="font-size:12px;color:var(--td-muted-2);">.json or .csv</span>
			<input type="file" accept=".json,.csv" on:change={(e) => (file = e.currentTarget.files?.[0] ?? null)} style="display:none;">
		</label>
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
				<div style="font-size:12.5px;color:var(--td-good-fg);margin-top:2px;">Deduplicated against your existing history ({job?.fetched} scanned).</div>
			</div>
		</div>
		<div style="display:flex;gap:8px;">
			<button class="btn btn-secondary" on:click={reset}>Import more</button>
			<a class="btn btn-primary" href="/history" style="text-decoration:none;">View history</a>
		</div>
	</div>
{/if}

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
