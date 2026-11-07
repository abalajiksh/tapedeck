<script lang="ts">
	// "What's on now" at one slot of a chain, and the "since when?" step.
	//
	// A slot is a step with options (HD 569, Hexa, S08). Switching never
	// retires anything: each option keeps the hours filed to it and says when
	// it came into use (its purchase date, else its first play). A switch can
	// be backdated; the plays from that moment up to the next recorded switch
	// of this step move, and the count is shown before anything is saved.
	//
	// It saves on its own (the switch, and options added or removed) and tells
	// the page with `changed`, which reloads.
	import { createEventDispatcher } from 'svelte';
	import { api, type ChainComponent, type Equipment, type FitOutcome, type SignalChain, type Slot } from '$lib/api';
	import { formatHours } from '$lib/utils';

	export let chain: SignalChain;
	export let position: number;
	/** The slot as the server describes it; null while the step is not a slot. */
	export let slot: Slot | null = null;
	export let equipment: Equipment[] = [];
	/** Hours of an option through this chain, from the gear breakdown. */
	export let hoursOf: (equipmentId: number) => number = () => 0;
	export let lastPlayEnd: number | null = null;

	const dispatch = createEventDispatcher<{ changed: string; close: void }>();

	$: step = (chain.components ?? [])[position] as ChainComponent | undefined;
	$: current = slot?.options.find((o) => o.equipment_id === slot?.on) ?? null;
	$: wiredGear = equipment.find((e) => e.id === step?.equipment_id);
	// Offered for adding: your gear of the same kind as what is wired here
	// first, then everything else, minus what is already an option.
	$: taken = new Set(slot ? slot.options.map((o) => o.equipment_id) : step?.equipment_id != null ? [step.equipment_id] : []);
	$: addable = equipment
		.filter((e) => !taken.has(e.id))
		.sort(
			(a, b) =>
				Number(b.equipment_type === wiredGear?.equipment_type) - Number(a.equipment_type === wiredGear?.equipment_type) ||
				a.name.localeCompare(b.name)
		);

	let pending: number | null = null;
	let when: 'now' | 'last' | 'pick' = 'now';
	let picked = '';
	let preview: FitOutcome | null = null;
	let lastPreview: FitOutcome | null = null;
	let busy = false;
	let error = '';
	let adding = '';

	const day = (ts: number) =>
		new Date(ts * 1000).toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' });
	const moment = (ts: number) =>
		new Date(ts * 1000).toLocaleString(undefined, { weekday: 'short', day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
	const nameOf = (id: number | null) =>
		slot?.options.find((o) => o.equipment_id === id)?.name ?? equipment.find((e) => e.id === id)?.name ?? 'that';
	function inUse(o: { purchased_at: number | null; first_play: number | null }) {
		if (o.purchased_at) return `in use since ${day(o.purchased_at)}`;
		if (o.first_play) return `in use since ${day(o.first_play)}`;
		return 'no plays yet';
	}
	function sub(o: Slot['options'][number]): string {
		const bits = [[o.brand, o.equipment_type].filter(Boolean).join(' · ')];
		bits.push(inUse(o));
		if (o.picked_by.length) bits.push(`picked by itself on ${o.picked_by.join(', ')}`);
		return bits.filter(Boolean).join(' · ');
	}

	$: pendingName = nameOf(pending);
	$: sinceTs =
		when === 'now' ? null : when === 'last' ? lastPlayEnd : picked ? Math.floor(new Date(picked).getTime() / 1000) : null;
	$: nowLocal = (() => {
		const d = new Date();
		d.setMinutes(d.getMinutes() - d.getTimezoneOffset());
		return d.toISOString().slice(0, 16);
	})();

	async function choose(id: number) {
		error = '';
		if (!slot || id === slot.on) {
			pending = null;
			return;
		}
		pending = id;
		when = 'now';
		picked = '';
		preview = null;
		lastPreview = null;
		if (lastPlayEnd != null) {
			try {
				lastPreview = await api.fitSlot(chain.id, { position, equipment_id: id, since: lastPlayEnd, dry_run: true });
			} catch {
				lastPreview = null;
			}
		}
	}

	// The count for whichever moment is chosen, from the same rule the switch
	// uses, so the number on the button is the number that moves.
	let previewKey = '';
	$: void refreshPreview(pending, when, sinceTs);
	async function refreshPreview(p: number | null, w: string, s: number | null) {
		const key = `${p}:${w}:${s}`;
		previewKey = key;
		if (p == null || (w === 'pick' && s == null)) {
			preview = null;
			return;
		}
		try {
			const out = await api.fitSlot(chain.id, { position, equipment_id: p, since: s, dry_run: true });
			if (previewKey === key) preview = out;
		} catch (e) {
			if (previewKey === key) {
				preview = null;
				error = e instanceof Error ? e.message : 'Could not count that';
			}
		}
	}

	async function confirmSwitch() {
		if (pending == null) return;
		busy = true;
		error = '';
		try {
			const out = await api.fitSlot(chain.id, { position, equipment_id: pending, since: sinceTs, dry_run: false });
			const moved = out.moved ? ` ${out.moved} ${out.moved === 1 ? 'play' : 'plays'} moved to it.` : '';
			dispatch(
				'changed',
				out.now_on
					? `${pendingName} is on now.${moved}`
					: `${pendingName} filed from ${moment(out.since)} to ${moment(out.until!)}.${moved}`
			);
			pending = null;
		} catch (e) {
			error = e instanceof Error ? e.message : 'That did not save';
		} finally {
			busy = false;
		}
	}

	/** Options are part of the chain's steps; the rest of the chain is sent
	 *  back as it is. */
	async function saveOptions(options: number[], note: string) {
		if (!step) return;
		busy = true;
		error = '';
		try {
			const components = (chain.components ?? []).map((c, i) => (i === position ? { ...c, options } : c));
			await api.updateChain(chain.id, {
				name: chain.name,
				description: chain.description ?? undefined,
				components,
				listening_context: chain.listening_context,
				icon: chain.icon ?? null
			});
			dispatch('changed', note);
		} catch (e) {
			error = e instanceof Error ? e.message : 'That did not save';
		} finally {
			busy = false;
		}
	}
	function addOption() {
		const id = Number(adding);
		if (!id || step?.equipment_id == null) return;
		const now = slot ? slot.options.map((o) => o.equipment_id) : [step.equipment_id];
		adding = '';
		saveOptions([...now, id], `${nameOf(id)} can be on here now.`);
	}
	function removeOption(id: number) {
		if (!slot) return;
		const rest = slot.options.map((o) => o.equipment_id).filter((x) => x !== id);
		saveOptions(rest.length >= 2 ? rest : [], `${nameOf(id)} is no longer offered here. Its hours stay.`);
	}
</script>

<div class="picker">
	{#if slot && current}
		{#if pending == null}
			<div class="cur">
				<span class="dot on" aria-hidden="true"></span>
				<div class="ct">
					<b>{current.name}</b>
					<small>
						{slot.on_since ? `On since ${moment(slot.on_since)}` : `On all along · ${inUse(current)}`}
					</small>
				</div>
			</div>
			<div class="opts" role="radiogroup" aria-label="What’s on at {step?.name}">
				{#each slot.options as o (o.equipment_id)}
					<div class="optrow">
						<button class="opt" class:on={o.equipment_id === slot.on} role="radio" aria-checked={o.equipment_id === slot.on}
							disabled={busy} on:click={() => choose(o.equipment_id)}>
							<span class="radio"></span>
							<span class="ct"><b>{o.name}</b><small>{sub(o)}</small></span>
							<span class="hrs">{hoursOf(o.equipment_id) > 0 ? `${formatHours(hoursOf(o.equipment_id))} here` : '0h'}</span>
						</button>
						{#if o.equipment_id !== slot.wired && o.equipment_id !== slot.on}
							<button class="x" title="Stop offering {o.name} here (its hours stay)" aria-label="Stop offering {o.name} here"
								disabled={busy} on:click={() => removeOption(o.equipment_id)}>✕</button>
						{/if}
					</div>
				{/each}
			</div>
		{:else}
			<div class="since">
				<h4>Since when has the {pendingName} been on?</h4>
				<p class="muted">
					Plays from that point move from {current.name} to the {pendingName}. Earlier plays stay where they are,
					and so does any switch you recorded after it.
				</p>
				<div class="opts" role="radiogroup" aria-label="Since when">
					<button class="opt" class:on={when === 'now'} role="radio" aria-checked={when === 'now'} on:click={() => (when = 'now')}>
						<span class="radio"></span><span class="ct"><b>Just now</b><small>Nothing earlier changes</small></span>
					</button>
					{#if lastPlayEnd != null}
						<button class="opt" class:on={when === 'last'} role="radio" aria-checked={when === 'last'} on:click={() => (when = 'last')}>
							<span class="radio"></span>
							<span class="ct">
								<b>Right after the last play here</b>
								<small>{moment(lastPlayEnd)}{lastPreview ? ` · moves ${lastPreview.moved} ${lastPreview.moved === 1 ? 'play' : 'plays'}` : ''}</small>
							</span>
						</button>
					{/if}
					<button class="opt" class:on={when === 'pick'} role="radio" aria-checked={when === 'pick'} on:click={() => (when = 'pick')}>
						<span class="radio"></span><span class="ct"><b>Pick a time…</b><small>For when you forgot to switch</small></span>
					</button>
				</div>
				{#if when === 'pick'}
					<label class="when">
						<span>On from</span>
						<input class="input" type="datetime-local" max={nowLocal} bind:value={picked} />
					</label>
				{/if}
				{#if preview}
					<p class="count">
						{#if preview.moved}
							Moves {preview.moved} {preview.moved === 1 ? 'play' : 'plays'} to the {pendingName}.
						{:else}
							Moves no plays; it counts from {when === 'now' ? 'now' : moment(preview.since)}.
						{/if}
						{#if preview.until}
							It stops at {moment(preview.until)}, the next switch you recorded here. That switch stays,
							so {current.name} is still on now.
						{/if}
					</p>
				{/if}
				<div class="acts">
					<button class="btn btn-secondary" disabled={busy} on:click={() => (pending = null)}>Cancel</button>
					<button class="btn btn-primary" disabled={busy || (when === 'pick' && sinceTs == null)} on:click={confirmSwitch}>
						{preview?.until ? `File as ${pendingName}` : `Switch to ${pendingName}`}
					</button>
				</div>
			</div>
		{/if}
	{/if}

	{#if pending == null && step?.equipment_id != null}
		<div class="add">
			<select class="input" bind:value={adding} aria-label="Gear that can also be on at {step.name}" disabled={busy}>
				<option value="">{slot ? '+ Other gear that can be here…' : '+ Gear you swap in here…'}</option>
				{#each addable as e (e.id)}
					<option value={String(e.id)}>{e.name}{e.brand ? ` · ${e.brand}` : ''}</option>
				{/each}
			</select>
			{#if adding}<button class="btn btn-secondary" disabled={busy} on:click={addOption}>Add</button>{/if}
		</div>
		{#if !slot}
			<p class="muted hint">
				Add the other headphones, cartridge or speakers you swap in at this step. Then pick which one is on, and
				every play from then is filed with it.
			</p>
		{/if}
	{/if}
	{#if error}<p class="err">{error}</p>{/if}
</div>

<style>
	.picker {
		display: flex;
		flex-direction: column;
		gap: 10px;
		margin-top: 8px;
	}
	.cur {
		display: flex;
		align-items: center;
		gap: 10px;
	}
	.dot {
		width: 10px;
		height: 10px;
		border-radius: 999px;
		background: var(--td-accent);
		flex: none;
	}
	.ct {
		display: flex;
		flex-direction: column;
		min-width: 0;
		flex: 1;
	}
	.ct b {
		font-weight: 600;
		font-size: 13.5px;
	}
	.ct small {
		font-size: 12px;
		color: var(--td-muted);
	}
	.opts {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}
	.optrow {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.opt {
		flex: 1;
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
	.opt.on {
		border-color: var(--td-accent);
	}
	.opt:disabled {
		cursor: default;
		opacity: 0.7;
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
	.hrs {
		font-size: 12.5px;
		color: var(--td-muted-2);
		font-variant-numeric: tabular-nums;
		white-space: nowrap;
	}
	.x {
		border: none;
		background: none;
		color: var(--td-muted-2);
		cursor: pointer;
		font-size: 13px;
		padding: 6px;
		border-radius: 8px;
	}
	.x:hover {
		color: var(--td-bad-fg);
	}
	.since h4 {
		margin: 0;
		font-size: 15px;
		font-weight: 700;
	}
	.since {
		display: flex;
		flex-direction: column;
		gap: 10px;
		padding: 12px 14px;
		border-radius: 16px;
		background: color-mix(in srgb, var(--td-accent) 7%, var(--td-surface));
		border: 1px solid color-mix(in srgb, var(--td-accent) 32%, transparent);
	}
	.muted {
		margin: 0;
		font-size: 12.5px;
		color: var(--td-muted);
		line-height: 1.45;
	}
	.hint {
		margin-top: -2px;
	}
	.when {
		display: flex;
		flex-direction: column;
		gap: 4px;
		max-width: 260px;
	}
	.when span {
		font-size: 12px;
		font-weight: 600;
		color: var(--td-muted);
	}
	.count {
		margin: 0;
		font-size: 13px;
		color: var(--td-text);
	}
	.acts {
		display: flex;
		justify-content: flex-end;
		gap: 8px;
		flex-wrap: wrap;
	}
	.add {
		display: flex;
		gap: 8px;
		align-items: center;
	}
	.add select {
		flex: 1;
		min-width: 0;
	}
	.err {
		margin: 0;
		font-size: 13px;
		color: var(--td-bad-fg);
	}
</style>
