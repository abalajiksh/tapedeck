<script lang="ts">
	// One question from the Signal Chains inbox, and its answer.
	//
	// Tapedeck asks only when a play would otherwise land in no chain, once
	// each, and the answer covers future plays too. Nothing is assigned or
	// grouped without a press here: a name match is a suggestion, ticked by
	// default and untickable.
	import { createEventDispatcher } from 'svelte';
	import { api, type Equipment, type InboxQuestion, type SignalChain } from '$lib/api';
	import { formatHours } from '$lib/utils';

	export let q: InboxQuestion;
	export let chains: SignalChain[] = [];
	export let equipment: Equipment[] = [];
	/** "2 of 5", when shown as the page's one card. */
	export let position = '';

	const dispatch = createEventDispatcher<{ done: string }>();
	let busy = false;
	let error = '';

	const SOURCE_TYPES = new Set(['phone', 'computer', 'dap', 'source', 'transport', 'turntable', 'radio', 'receiver']);
	$: boxes = equipment.filter((e) => SOURCE_TYPES.has(e.equipment_type));

	// same_box
	let ticked = new Set<string>((q.reporters ?? []).map((r) => `${r.kind}:${r.id}`));
	function tick(k: string) {
		ticked.has(k) ? ticked.delete(k) : ticked.add(k);
		ticked = ticked;
	}
	// which_box
	let boxId: number | null = null;
	let alias = q.kind === 'which_box' ? (q.reporters?.[0]?.name ?? '') : '';
	// where
	let chainId: number | null = null;
	let backfill = true;

	const when = (ts?: number) =>
		ts
			? new Date(ts * 1000).toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' })
			: '';
	$: from = q.output
		? `“${q.output}”${q.device ? ` on ${q.device.name}` : q.token ? ` from ${q.token.name}` : ''}`
		: q.device
			? `“${q.device.name}”`
			: q.token
				? `the token “${q.token.name}”`
				: `“${q.source_name}”`;
	// `ingest:<client>` is how the ingest API files a listen, not a name
	// anyone chose; the token that sent it is the useful thing to say.
	$: via = q.source_name?.startsWith('ingest')
		? q.token
			? `, sent with the token “${q.token.name}”`
			: ''
		: q.source_name
			? `, through ${q.source_name}`
			: '';
	$: ruleNote = {
		device_output: `Future plays out of ${q.output ? `“${q.output}”` : 'that output'} on this box go there too.`,
		output: `Future plays out of ${q.output ? `“${q.output}”` : 'that output'} go there too, from anything that reports it.`,
		token: `Future plays sent with this token go there too.`,
		device: `Future plays from this player go there too, unless something more specific says otherwise.`,
		none: `Nothing in these plays says where they came from, so future ones can’t find a chain on their own. Only these can be filed.`
	}[q.rule ?? 'none'];
	$: sortedChains = [...chains].sort((a, b) => b.total_hours - a.total_hours || a.name.localeCompare(b.name));

	async function run(fn: () => Promise<string>) {
		busy = true;
		error = '';
		try {
			dispatch('done', await fn());
		} catch (e) {
			error = e instanceof Error ? e.message : 'That did not save';
		} finally {
			busy = false;
		}
	}
	const notNow = () =>
		run(async () => {
			await api.dismissInbox(q.key);
			return 'Put off for a month.';
		});
	const saveBox = () =>
		run(async () => {
			const picked = (q.reporters ?? []).filter((r) => ticked.has(`${r.kind}:${r.id}`));
			await api.answerInboxBox({
				equipment_id: q.equipment_id!,
				device_ids: picked.filter((r) => r.kind === 'device').map((r) => r.id),
				token_ids: picked.filter((r) => r.kind === 'token').map((r) => r.id)
			});
			return `${picked.length} ${picked.length === 1 ? 'entry is' : 'entries are'} now ${q.equipment_name}.`;
		});
	const saveWhich = () =>
		run(async () => {
			const r = q.reporters![0];
			await api.answerInboxBox({
				equipment_id: boxId!,
				device_ids: r.kind === 'device' ? [r.id] : [],
				token_ids: r.kind === 'token' ? [r.id] : [],
				alias: alias.trim() || null
			});
			return `${r.name} is now ${boxes.find((b) => b.id === boxId)?.name ?? 'that gear'}.`;
		});
	const saveWhere = () =>
		run(async () => {
			const out = await api.answerInboxWhere({
				device_id: q.device?.id ?? null,
				token_id: q.token?.id ?? null,
				output: q.output ?? null,
				source_name: q.source_name ?? '',
				since: q.since ?? 0,
				chain_id: chainId!,
				backfill
			});
			const name = chains.find((c) => c.id === chainId)?.name ?? 'that chain';
			const filed = out.moved ? `, and ${out.moved} ${out.moved === 1 ? 'play is' : 'plays are'} filed there` : '';
			return out.rule === 'none' ? `Filed ${out.moved} in ${name}.` : `Done. Plays from ${from} go to ${name}${filed}.`;
		});
</script>

<section class="q" aria-label="A question from Tapedeck">
	{#if q.kind === 'same_box'}
		<h3>Are these all your {q.equipment_name}?</h3>
		<p class="muted">
			Each app that reports plays names a box its own way. These carry a name or model number of
			your {q.equipment_name}. Tick the ones that are it and they become one box on the graph.
			Hours already logged stay where they are.
		</p>
		<div class="ticks">
			{#each q.reporters ?? [] as r (`${r.kind}:${r.id}`)}
				<button class="chip" class:on={ticked.has(`${r.kind}:${r.id}`)} aria-pressed={ticked.has(`${r.kind}:${r.id}`)}
					on:click={() => tick(`${r.kind}:${r.id}`)}>
					<span class="box">{ticked.has(`${r.kind}:${r.id}`) ? '✓' : ''}</span>
					<span class="ct"><b>{r.name}</b><small>{r.sub}</small></span>
				</button>
			{/each}
		</div>
		<div class="acts">
			<span class="muted grow">{ticked.size} of {(q.reporters ?? []).length} ticked.</span>
			<button class="btn btn-secondary" disabled={busy} on:click={notNow}>Not now</button>
			<button class="btn btn-primary" disabled={busy || !ticked.size} on:click={saveBox}>Make them one {q.equipment_name}</button>
		</div>
	{:else if q.kind === 'which_box'}
		{@const r = q.reporters?.[0]}
		<h3>Which of your gear is “{r?.name}”?</h3>
		<p class="muted">
			It sent plays that landed in no chain. Say which box it is and it joins that box on the graph;
			the name below is remembered, so the next app reporting it is suggested as the same box.
		</p>
		<div class="opts">
			{#each boxes as b (b.id)}
				<button class="opt" class:on={boxId === b.id} aria-pressed={boxId === b.id} on:click={() => (boxId = b.id)}>
					<span class="radio"></span>
					<span class="ct"><b>{b.name}</b><small>{[b.brand, b.model].filter(Boolean).join(' ') || b.equipment_type}</small></span>
				</button>
			{/each}
		</div>
		<label class="alias">
			<span>Remember it as</span>
			<input class="input" bind:value={alias} placeholder="A model number, say" />
		</label>
		<div class="acts">
			<span class="grow"></span>
			<button class="btn btn-secondary" disabled={busy} on:click={notNow}>Not one of mine</button>
			<button class="btn btn-primary" disabled={busy || boxId == null} on:click={saveWhich}>Save</button>
		</div>
	{:else}
		<h3>{q.plays} {q.plays === 1 ? 'play' : 'plays'} from {from} landed nowhere. Where were you?</h3>
		<p class="muted">
			{when(q.first)}{q.last && q.last !== q.first ? ` to ${when(q.last)}` : ''}{via}.
			{ruleNote}
		</p>
		<div class="opts">
			{#each sortedChains as c (c.id)}
				<button class="opt" class:on={chainId === c.id} aria-pressed={chainId === c.id} on:click={() => (chainId = c.id)}>
					<span class="radio"></span>
					<span class="ct">
						<b>{c.name}</b>
						<small>{(c.components ?? []).map((s) => s.name).filter(Boolean).join(' → ') || 'No steps yet'}{c.total_hours > 0 ? ` · ${formatHours(c.total_hours)}` : ''}</small>
					</span>
				</button>
			{:else}
				<p class="muted">You have no chains yet. Make one on Signal Chains, then come back.</p>
			{/each}
		</div>
		<div class="acts">
			<button class="check" class:on={backfill} aria-pressed={backfill} on:click={() => (backfill = !backfill)}>
				<span class="box">{backfill ? '✓' : ''}</span>Also file the {q.plays} {q.plays === 1 ? 'play' : 'plays'} counted here
			</button>
			<span class="grow"></span>
			<button class="btn btn-secondary" disabled={busy} on:click={notNow}>Leave them unfiled</button>
			<button class="btn btn-primary" disabled={busy || chainId == null || (q.rule === 'none' && !backfill)} on:click={saveWhere}>Save</button>
		</div>
	{/if}
	{#if error}<p class="err">{error}</p>{/if}
	{#if position}<span class="pos">{position}</span>{/if}
</section>

<style>
	.q {
		position: relative;
		display: flex;
		flex-direction: column;
		gap: 12px;
		padding: 18px 20px;
		border-radius: 20px;
		background: color-mix(in srgb, var(--td-accent) 7%, var(--td-surface));
		border: 1px solid color-mix(in srgb, var(--td-accent) 32%, transparent);
	}
	h3 {
		margin: 0;
		font-size: 17px;
		font-weight: 700;
		padding-right: 70px;
	}
	.muted {
		margin: 0;
		font-size: 13px;
		color: var(--td-muted);
		line-height: 1.45;
		max-width: 75ch;
	}
	.ticks {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
		gap: 8px;
	}
	.opts {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-height: 300px;
		overflow-y: auto;
	}
	.chip,
	.opt {
		display: flex;
		align-items: center;
		gap: 10px;
		text-align: left;
		padding: 9px 12px;
		border-radius: 14px;
		border: 1px solid var(--td-border);
		background: var(--td-surface);
		font: inherit;
		color: inherit;
		cursor: pointer;
	}
	.chip.on,
	.opt.on {
		border-color: var(--td-accent);
	}
	.ct {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.ct b {
		font-weight: 600;
		font-size: 13.5px;
	}
	.ct small {
		font-size: 12px;
		color: var(--td-muted);
	}
	.box {
		flex: none;
		width: 20px;
		height: 20px;
		border-radius: 6px;
		border: 1.5px solid var(--td-muted-3);
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: 12px;
		color: var(--td-on-accent);
	}
	.on > .box {
		background: var(--td-accent);
		border-color: var(--td-accent);
	}
	.radio {
		flex: none;
		width: 18px;
		height: 18px;
		border-radius: 999px;
		border: 1.5px solid var(--td-muted-3);
	}
	.opt.on .radio {
		border: 5px solid var(--td-accent);
	}
	.alias {
		display: flex;
		flex-direction: column;
		gap: 6px;
		max-width: 360px;
	}
	.alias span {
		font-size: 12px;
		font-weight: 600;
		color: var(--td-muted);
	}
	.acts {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.grow {
		flex: 1 1 200px;
	}
	.check {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		border: none;
		background: none;
		font: inherit;
		font-size: 13px;
		color: var(--td-text);
		cursor: pointer;
		padding: 4px 0;
	}
	.err {
		margin: 0;
		font-size: 13px;
		color: var(--td-bad-fg);
	}
	.pos {
		position: absolute;
		top: 16px;
		right: 18px;
		font-size: 12px;
		color: var(--td-muted-2);
	}
</style>
