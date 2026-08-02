<script lang="ts">
	import { api, type Session, type Scrobble, type SignalChain } from '$lib/api';
	import { formatDuration, timeAgo } from '$lib/utils';
	import ScrobbleRow from './ScrobbleRow.svelte';
	import { createEventDispatcher } from 'svelte';

	export let session: Session;
	export let chains: SignalChain[] = [];
	/** Selection is for merging — several sittings folded into one. */
	export let selectable = false;
	export let selected = false;

	const dispatch = createEventDispatcher<{ toggle: Session; changed: void }>();

	let open = false;
	let scrobbles: Scrobble[] = [];
	let loadingRows = false;
	let busy = false;
	/** The split control only appears once you're looking inside. */
	let splitting = false;

	// Listens are fetched when the card is opened, not with the list — a page of
	// sessions is cheap, forty of their tracklists is not.
	async function toggleOpen() {
		open = !open;
		if (open && !scrobbles.length) {
			loadingRows = true;
			try {
				scrobbles = (await api.getSession(session.id)).scrobbles;
			} catch { /* leave it closed-empty rather than erroring the page */ }
			loadingRows = false;
		}
	}

	async function setChain(value: string) {
		busy = true;
		try {
			const r = await api.updateSession(session.id, { chain_id: value ? Number(value) : null });
			session = r.session;
			// The listens carry the chain too, so anything showing them is stale.
			scrobbles = [];
			if (open) {
				scrobbles = (await api.getSession(session.id)).scrobbles;
			}
			dispatch('changed');
		} catch (e) { alert('Failed to set chain: ' + e); }
		busy = false;
	}

	async function splitAt(s: Scrobble) {
		busy = true;
		try {
			await api.splitSession(session.id, s.id);
			splitting = false;
			dispatch('changed');
		} catch (e) { alert('Split failed: ' + String(e).replace(/^API \d+: /, '')); }
		busy = false;
	}

	$: span = (() => {
		const start = new Date(session.started_at * 1000);
		const end = session.ended_at ? new Date(session.ended_at * 1000) : null;
		const t = (d: Date) => d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
		const day = start.toLocaleDateString('en-GB', { weekday: 'short', day: 'numeric', month: 'short' });
		return end ? `${day} · ${t(start)}–${t(end)}` : `${day} · ${t(start)}`;
	})();

	// The listening span, not the sum of track lengths — a session is a stretch
	// of time, and gaps inside it are part of what happened.
	$: elapsed = session.ended_at ? session.ended_at - session.started_at : session.total_duration;
	$: quality = session.avg_quality_score ? Math.round(session.avg_quality_score) : null;
</script>

<div class="card" class:selected>
	<div class="head">
		{#if selectable}
			<input type="checkbox" checked={selected} on:change={() => dispatch('toggle', session)}
				aria-label="Select session for merging"
				style="width:18px;height:18px;flex:none;accent-color:var(--td-accent);cursor:pointer;">
		{/if}
		<button class="summary" on:click={toggleOpen} aria-expanded={open}>
			<svg class="chev" class:open viewBox="0 0 24 24" width="16" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>
			<span style="flex:1;min-width:0;">
				<span class="title">{session.title ?? span}</span>
				<span class="meta">
					{session.track_count} track{session.track_count === 1 ? '' : 's'}
					· {formatDuration(elapsed)}
					{#if session.chain_name}· {session.chain_name}{/if}
					{#if quality}· {quality}/100{/if}
					{#if session.token_name}· {session.token_name}{/if}
				</span>
			</span>
			<span class="ago">{timeAgo(session.started_at)}</span>
		</button>
	</div>

	{#if open}
		<div class="body">
			<div class="actions">
				<select class="input" style="width:auto;min-width:170px;font-size:12.5px;"
					value={session.chain_id ?? ''} disabled={busy}
					on:change={(e) => setChain(e.currentTarget.value)} aria-label="Signal chain for this session">
					<option value="">— no chain —</option>
					{#each chains as c}<option value={c.id}>{c.name}</option>{/each}
				</select>
				<span style="font-size:12px;color:var(--td-muted-3);">
					Retags every listen in this sitting.
				</span>
				<span style="flex:1;"></span>
				{#if scrobbles.length > 1}
					<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => (splitting = !splitting)}>
						{splitting ? 'Cancel split' : 'Split…'}
					</button>
				{/if}
			</div>

			{#if splitting}
				<p class="hint">Pick the track that starts the second session.</p>
			{/if}

			{#if loadingRows}
				<div style="display:flex;justify-content:center;padding:24px;"><div class="spin"></div></div>
			{:else}
				{#each scrobbles as s, i (s.id)}
					<div class="row-wrap">
						<ScrobbleRow scrobble={s} />
						{#if splitting && i > 0}
							<button class="split-here" on:click={() => splitAt(s)} disabled={busy}>
								Split here
							</button>
						{/if}
					</div>
				{/each}
			{/if}
		</div>
	{/if}
</div>

<style>
	.card {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 18px;
		overflow: hidden;
		margin-bottom: 12px;
	}
	.card.selected { border-color: var(--td-accent); }
	.head { display: flex; align-items: center; gap: 10px; padding-left: 14px; }
	.summary {
		flex: 1;
		min-width: 0;
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 14px 16px 14px 2px;
		border: none;
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
	}
	.summary:hover .title { color: var(--td-accent); }
	.chev { flex: none; color: var(--td-muted-3); transition: transform .15s; }
	.chev.open { transform: rotate(90deg); }
	.title {
		display: block;
		font-size: 14.5px;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.meta {
		display: block;
		font-size: 12.5px;
		color: var(--td-muted-2);
		margin-top: 2px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.ago { flex: none; font-size: 12px; color: var(--td-muted-3); }
	.body { border-top: 1px solid var(--td-border); }
	.actions {
		display: flex;
		align-items: center;
		gap: 10px;
		flex-wrap: wrap;
		padding: 12px 16px;
		background: var(--td-surface2);
	}
	.hint {
		margin: 0;
		padding: 8px 16px;
		font-size: 12.5px;
		color: var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 8%, transparent);
	}
	.row-wrap { position: relative; }
	/* Sits over the row it would cut *before*, so the boundary you're choosing is
	   the one you can see. */
	.split-here {
		position: absolute;
		top: -11px;
		right: 16px;
		z-index: 2;
		border: 1px solid var(--td-accent);
		background: var(--td-surface);
		color: var(--td-accent);
		font: inherit;
		font-size: 11.5px;
		font-weight: 600;
		padding: 3px 10px;
		border-radius: 999px;
		cursor: pointer;
	}
	.split-here:hover { background: var(--td-accent); color: #fbf5ea; }
	.spin {
		width: 18px; height: 18px; border: 2px solid var(--td-track);
		border-top-color: var(--td-accent); border-radius: 50%;
		animation: td-spin 0.7s linear infinite;
	}
</style>
