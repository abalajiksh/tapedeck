<script lang="ts">
	import { api, type Session, type Scrobble, type SignalChain, type ChainVariant } from '$lib/api';
	import { formatDuration, timeAgo } from '$lib/utils';
	import ScrobbleRow from './ScrobbleRow.svelte';
	import { createEventDispatcher } from 'svelte';

	export let session: Session;
	/** Decks this person is patched with, loaded once by the page. Empty means
	 *  there is nobody to hand a sitting to, and the button does not render. */
	export let peers: { username: string; display_name: string | null }[] = [];
	export let chains: SignalChain[] = [];
	/** Variants of those chains, so a sitting can say *which* setup it was —
	 *  the same chain with the other IEMs in, or the other cartridge fitted. */
	export let variants: ChainVariant[] = [];
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
	let handing = false;
	let handTo = '';
	let handNote = '';
	// No default. The two do different things to somebody else's history, and
	// a pre-selected radio is a decision made for the user by whoever wrote
	// the markup — which is exactly the guess the endpoint refuses to make.
	let handKind: 'spool' | 'recommendation' | '' = '';
	let handResult = '';

	/**
	 * Hand this sitting to somebody patched with you.
	 *
	 * Copies nothing — the recipient gets a document rendered from these
	 * listens, which enters no history of theirs. That is the whole difference
	 * from a Shared Spool, where they were in the room and the plays really are
	 * offered to their deck.
	 */
	async function handOver() {
		if (!session.uuid || !handTo || !handKind) return;
		busy = true;
		try {
			const r = await api.handOverSitting({
				session: session.uuid,
				to: handTo,
				kind: handKind,
				note: handNote.trim() || undefined
			});
			// Reported rather than silently succeeding: for a reconstructed
			// sitting the interesting number is what was *not* offered, and the
			// sender is the only one who can fix it.
			if (r.kind === 'spool') {
				handResult =
					`${r.offered ?? 0} ${r.offered === 1 ? 'play' : 'plays'} sent to confirm` +
					(r.withheld
						? ` · ${r.withheld} withheld — they ran through chains you don't share`
						: '');
			} else {
				handResult = 'Passed on. Nothing entered their history.';
			}
			handTo = '';
			handKind = '';
			handNote = '';
		} catch (e) {
			alert(e instanceof Error ? e.message : 'Could not do that');
		}
		busy = false;
	}

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

	// ── Naming a sitting, and writing about it ──
	//
	// A session is the closest thing this app has to "an evening", and the two
	// things you want to say about one are what it *was* and what happened. The
	// name is not a liner note: a liner note is about a record and lives on an
	// entity, while this is about the sitting.
	let editing = false;
	let draftTitle = '';
	let draftNotes = '';

	function startEdit() {
		draftTitle = session.title ?? '';
		draftNotes = session.notes ?? '';
		editing = true;
	}

	async function saveMeta() {
		busy = true;
		try {
			// Empty means cleared, which the endpoint distinguishes from absent —
			// so a name can actually be removed rather than only replaced.
			const r = await api.updateSession(session.id, {
				title: draftTitle.trim() || null,
				notes: draftNotes.trim() || null
			});
			session = r.session;
			editing = false;
			dispatch('changed');
		} catch (e) { alert('Failed to save: ' + e); }
		busy = false;
	}

	/** Variants belong to one chain, so the picker only offers this one's. */
	$: ownVariants = variants.filter((v) => v.chain_id === session.chain_id);

	async function setVariant(value: string) {
		busy = true;
		try {
			// The chain goes with it: a variant belongs to exactly one chain, and
			// the endpoint rewrites both together for that reason.
			const r = await api.updateSession(session.id, {
				chain_id: session.chain_id,
				chain_variant_id: value ? Number(value) : null
			});
			session = r.session;
			scrobbles = [];
			if (open) scrobbles = (await api.getSession(session.id)).scrobbles;
			dispatch('changed');
		} catch (e) { alert('Failed to set the variant: ' + e); }
		busy = false;
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
					{#if session.chain_name}
						· {session.chain_name}{#if session.chain_variant_name} ({session.chain_variant_name}){/if}
					{/if}
					{#if session.notes}· <span class="hasnote" title={session.notes}>noted</span>{/if}
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
				{#if ownVariants.length}
					<!-- One field rather than a second chain. Two cartridges on one
					     turntable, or the same commute on the other IEMs, are the
					     same path with one step swapped — and this is what makes
					     the hours land on the gear that was actually fitted. -->
					<select class="input" style="width:auto;min-width:150px;font-size:12.5px;"
						value={session.chain_variant_id ?? ''} disabled={busy}
						on:change={(e) => setVariant(e.currentTarget.value)}
						aria-label="Which variant of that chain">
						<option value="">— as the chain is —</option>
						{#each ownVariants as v}<option value={v.id}>{v.name}</option>{/each}
					</select>
				{/if}
				<span style="font-size:12px;color:var(--td-muted-3);">
					Retags every listen in this sitting.
				</span>
				<span style="flex:1;"></span>
				<button class="btn btn-ghost" style="font-size:12.5px;" on:click={startEdit}>
					{session.title || session.notes ? 'Edit note' : 'Name it…'}
				</button>
				{#if scrobbles.length > 1}
					<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => (splitting = !splitting)}>
						{splitting ? 'Cancel split' : 'Split…'}
					</button>
				{/if}
				<!--
					Only when there is somebody to hand it to. `peers` is loaded
					once by the page rather than per card, and an empty list means
					no accepted patch — in which case the server would refuse
					anyway, so offering the button would be offering a 403.
				-->
				{#if session.uuid && peers.length}
					<button class="btn btn-ghost" style="font-size:12.5px;" on:click={() => (handing = !handing)}>
						{handing ? 'Cancel' : 'Hand over…'}
					</button>
				{/if}
			</div>

			{#if handing && session.uuid}
				<div class="edit">
					<!--
						**The two gestures, stated before anything else.** They write
						different things to another person's permanent record, so
						neither is pre-selected and the copy leads with the
						difference rather than with the mechanism.
					-->
					<div class="kindpick">
						<button
							class="kindopt"
							class:on={handKind === 'spool'}
							aria-pressed={handKind === 'spool'}
							on:click={() => (handKind = 'spool')}
						>
							<span class="kindname">They were listening with me</span>
							<span class="kindsub">
								The Shared Spool neither of you started. The plays land in their inbox to
								confirm and <strong>enter their history</strong>, exactly as a live sitting
								would have.
							</span>
						</button>
						<button
							class="kindopt"
							class:on={handKind === 'recommendation'}
							aria-pressed={handKind === 'recommendation'}
							on:click={() => (handKind = 'recommendation')}
						>
							<span class="kindname">Just showing them</span>
							<span class="kindsub">
								They weren't there. They see a record of what you played and
								<strong>nothing enters their history</strong>.
							</span>
						</button>
					</div>
					<p style="margin:0 0 11px;font-size:12px;color:var(--td-muted-2);text-wrap:pretty;">
						Either way, only listens through chains you share are passed on — the rest are
						counted so nobody is misled about what is missing.
					</p>
					<div class="field">
						<label for="hto-{session.id}">Hand it to</label>
						<select id="hto-{session.id}" class="input" bind:value={handTo}>
							<option value="">— pick a deck —</option>
							{#each peers as p (p.username)}
								<option value={p.username}>{p.display_name || p.username}</option>
							{/each}
						</select>
					</div>
					<!-- Only the document carries a note. A reconstructed sitting is
					     a set of captures, and there is nowhere on one to put a
					     sentence — so offering the field would be offering a box
					     whose contents are thrown away. -->
					{#if handKind !== 'spool'}
						<div class="field">
							<label for="hnote-{session.id}">Why <span style="opacity:.6">(optional)</span></label>
							<input id="hnote-{session.id}" class="input" bind:value={handNote}
								maxlength="500" placeholder="the run of Talk Talk in the middle" />
						</div>
					{/if}
					{#if handResult}
						<div class="handresult">{handResult}</div>
					{/if}
					<div style="display:flex;gap:8px;justify-content:flex-end;margin-top:11px;">
						<button class="btn btn-secondary" style="font-size:12.5px;" on:click={() => (handing = false)}>
							Cancel
						</button>
						<button class="btn btn-primary" style="font-size:12.5px;"
							disabled={busy || !handTo || !handKind} on:click={handOver}>
							{busy ? 'Sending…' : handKind === 'recommendation' ? 'Show them' : 'Send it over'}
						</button>
					</div>
				</div>
			{/if}

			{#if editing}
				<div class="edit">
					<div class="field">
						<label for="sn-{session.id}">What this sitting was</label>
						<input id="sn-{session.id}" class="input" bind:value={draftTitle}
							maxlength="120" placeholder="Sunday morning, new cartridge" />
					</div>
					<div class="field">
						<label for="sx-{session.id}">Notes</label>
						<textarea id="sx-{session.id}" class="input" rows="3" bind:value={draftNotes}
							placeholder="First listen on the microlinear. Bass is tighter."
							style="resize:vertical;font-family:inherit;"></textarea>
						<p class="hint">
							About the sitting, not about a record — what you thought of the
							record itself belongs in Liner Notes.
						</p>
					</div>
					<div style="display:flex;gap:8px;">
						<button class="btn btn-primary" style="font-size:12.5px;" on:click={saveMeta} disabled={busy}>
							{busy ? 'Saving…' : 'Save'}
						</button>
						<button class="btn btn-secondary" style="font-size:12.5px;" on:click={() => (editing = false)}>
							Cancel
						</button>
					</div>
				</div>
			{:else if session.notes}
				<p class="note">{session.notes}</p>
			{/if}

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
	.hasnote { color: var(--td-accent); }
	/* The two gestures. Cards rather than radios: the difference is in the
	   sentence under each name, and a radio label that long reads as a wall of
	   text with a dot in front of it. */
	.kindpick { display: flex; flex-direction: column; gap: 8px; margin-bottom: 12px; }
	.kindopt {
		display: flex;
		flex-direction: column;
		gap: 3px;
		text-align: left;
		padding: 11px 13px;
		border-radius: 13px;
		border: 1.5px solid var(--td-border);
		background: transparent;
		color: var(--td-text);
		font-family: inherit;
		cursor: pointer;
	}
	.kindopt:hover { border-color: var(--td-muted-3); }
	.kindopt.on {
		border-color: var(--td-accent);
		background: color-mix(in srgb, var(--td-accent) 10%, transparent);
	}
	.kindname { font-size: 13.5px; font-weight: 600; }
	.kindsub { font-size: 12px; color: var(--td-muted); line-height: 1.45; text-wrap: pretty; }
	.handresult {
		margin-top: 11px;
		font-size: 12.5px;
		color: var(--td-text);
		background: color-mix(in srgb, var(--td-accent) 10%, transparent);
		border-left: 3px solid var(--td-accent);
		border-radius: 0 12px 12px 0;
		padding: 9px 12px;
	}

	.edit {
		display: flex; flex-direction: column; gap: 12px;
		padding: 14px; margin: 4px 0 12px;
		border: 1px solid var(--td-border); border-radius: 14px;
		background: var(--td-surface2, var(--td-panel2));
	}
	.edit .field { display: flex; flex-direction: column; gap: 5px; }
	.edit label { font-size: 12.5px; color: var(--td-muted); }
	.hint { font-size: 12px; color: var(--td-muted-3); margin: 0; }
	.note {
		margin: 4px 0 12px; padding: 10px 14px;
		border-left: 2px solid var(--td-accent); border-radius: 0 8px 8px 0;
		background: var(--td-surface2, var(--td-panel2));
		font-size: 13px; color: var(--td-text); white-space: pre-wrap;
	}
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
