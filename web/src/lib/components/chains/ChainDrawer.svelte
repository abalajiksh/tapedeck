<script lang="ts">
	// One chain, opened: the route and what's on now at its slots, how plays
	// find it, the hours on its gear, and who sees what played through it.
	//
	// The design's "Place" layout. A gear step opens "what's on now" in place
	// (`WhatsOnPicker`): pick which option is on, since when, and which gear
	// can be on there at all. "Setup" is not a word on this screen any more;
	// the only named setups left are ones that add or remove a box, listed at
	// the bottom as they were.
	//
	// It owns no state that outlives it: every action goes back to the page,
	// which holds the dialogs and the data, and `changed` asks it to reload.
	import { createEventDispatcher } from 'svelte';
	import type { Binding, ChainVariant, Device, Equipment, Room, SignalChain, WhatsOn } from '$lib/api';
	import ChainIcon from '$lib/components/ChainIcon.svelte';
	import { formatHours } from '$lib/utils';
	import { COLUMNS, columnOf, roomGlyph } from './layout';
	import RoomIcon from './RoomIcon.svelte';
	import { lastUsedPhrase, variantLabel } from './format';
	import WhatsOnPicker from './WhatsOnPicker.svelte';

	export let chain: SignalChain;
	export let variants: ChainVariant[] = [];
	export let equipment: Equipment[] = [];
	export let devices: Device[] = [];
	export let bindings: Binding[] = [];
	export let lastUsed: number | null = null;
	export let sharing = false;
	/** Another chain differs from this one at one box, so it can merge in. */
	export let canMerge = false;
	export let rooms: Room[] = [];
	/** A rooms change is in flight. */
	export let movingRooms = false;
	/** This chain's slots, from `GET /chains/{id}/whats-on`; null while loading. */
	export let whatsOn: WhatsOn | null = null;
	/** What the last change said, shown under the route. */
	export let notice = '';

	const dispatch = createEventDispatcher<{
		close: void;
		share: boolean;
		removeVariant: ChainVariant;
		merge: void;
		delete: void;
		duplicate: void;
		edit: void;
		rooms: number[];
		changed: string;
	}>();

	$: inRooms = rooms.filter((r) => r.chain_ids.includes(chain.id)).map((r) => r.id);
	function toggleRoom(id: number) {
		dispatch('rooms', inRooms.includes(id) ? inRooms.filter((x) => x !== id) : [...inRooms, id]);
	}

	$: steps = chain.components ?? [];
	$: mine = variants.filter((v) => v.chain_id === chain.id);
	// Named setups slots could not take in (they add or remove a box). Those
	// that change no gear are offered as session titles in the inbox.
	$: older = mine.filter(
		(v) => !v.auto && (v.overrides ?? []).some((o) => o.equipment_id !== steps[o.position]?.equipment_id)
	);
	$: slotAt = (i: number) => whatsOn?.slots.find((s) => s.position === i) ?? null;
	$: onName = (i: number) => {
		const sl = slotAt(i);
		return sl ? (sl.options.find((o) => o.equipment_id === sl.on)?.name ?? steps[i]?.name) : steps[i]?.name;
	};
	let openPos: number | null = null;
	// A different chain closes whatever picker was open.
	$: chain.id, (openPos = null);

	/** Hours of a piece of gear through this chain, from the same breakdown
	 *  the gear tiles use, so the two cannot disagree. */
	$: hoursOf = (id: number) =>
		(equipment.find((e) => e.id === id)?.sources ?? [])
			.filter((s) => s.chain_id === chain.id)
			.reduce((a, s) => a + s.hours, 0);
	const CONTEXT_TAG: Record<string, string> = {
		active: 'tag-accent-2',
		'active-mobile': 'tag-accent-2',
		passive: 'tag-accent'
	};

	$: routing = [
		...bindings
			.filter((b) => b.chain_id === chain.id)
			.map((b) => ({
				key: `b${b.id}`,
				label: b.identifier,
				kind: b.interface ?? 'output',
				says:
					b.chain_variant_id != null
						? `Comes here and puts on ${(() => {
								const v = mine.find((x) => x.id === b.chain_variant_id);
								return v ? variantLabel(v, equipment) : 'other gear';
							})()} by itself`
						: whatsOn?.slots.length
							? 'Comes here, on whatever is on now'
							: 'Comes here'
			})),
		...devices
			.filter((d) => d.default_chain_id === chain.id)
			.map((d) => ({
				key: `d${d.id}`,
				label: d.name ?? d.machine_id,
				kind: d.platform ?? 'machine',
				says: 'Comes here when nothing more specific says where'
			}))
	];
	// Hours per piece of gear on *this* chain, from the same breakdown the
	// equipment table uses, so the two cannot disagree.
	$: gearHours = equipment
		.map((e) => ({
			name: e.name,
			hours: (e.sources ?? []).filter((s) => s.chain_id === chain.id).reduce((a, s) => a + s.hours, 0)
		}))
		.filter((g) => g.hours > 0)
		.sort((a, b) => b.hours - a.hours);

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') dispatch('close');
	}
</script>

<svelte:window on:keydown={onKey} />

<div class="back" on:click={() => dispatch('close')} on:keydown={() => {}} role="presentation"></div>
<aside class="drawer" aria-label="{chain.name}">
	<header class="dhead">
		<span class="dicon" aria-hidden="true"><ChainIcon name={chain.icon} size={22} title="" /></span>
		<div class="dtitle">
			<h2 class="h-display">{chain.name}</h2>
			<p class="dsub">
				{formatHours(chain.total_hours)} on this chain · {lastUsedPhrase(lastUsed)}
			</p>
			{#if chain.description}<p class="ddesc">{chain.description}</p>{/if}
			<span class="tag {CONTEXT_TAG[chain.listening_context] ?? 'tag-neutral'} dctx">{chain.listening_context}</span>
		</div>
		<button class="btn btn-secondary dclose" aria-label="Close" on:click={() => dispatch('close')}>
			<svg viewBox="0 0 24 24" width="18" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
		</button>
	</header>

	<div class="dbody">
		<section aria-label="The route">
			<div class="kicker">The route</div>
			{#each steps as s, i}
				{@const sl = slotAt(i)}
				<div class="step">
					<div class="rail">
						<span class="rdot"></span>
						{#if i < steps.length - 1}<span class="rline"></span>{/if}
					</div>
					<div class="sbox">
						{#if s.equipment_id != null}
							<button class="sbody" aria-expanded={openPos === i} on:click={() => (openPos = openPos === i ? null : i)}
								title={sl ? 'Change what’s on' : 'Gear you swap in here'}>
								<span class="srole">{s.slot_name ?? COLUMNS[columnOf(s.type)]}{sl ? ' · what’s on now' : ''}</span>
								<span class="sname">
									{#if sl}<span class="pill">{onName(i)} ▾</span>{:else}{s.name}{/if}
									<span class="shint" aria-hidden="true">{sl ? 'change' : '+ swaps'}</span>
								</span>
								{#if sl && sl.options.length > 1}
									<span class="sdetail">or {sl.options.filter((o) => o.equipment_id !== sl.on).map((o) => o.name).join(', ')}</span>
								{:else if s.detail}<span class="sdetail">{s.detail}</span>{/if}
							</button>
							{#if openPos === i}
								<WhatsOnPicker {chain} position={i} slot={sl} {equipment} {hoursOf}
									lastPlayEnd={whatsOn?.last_play_end ?? null}
									on:changed={(e) => dispatch('changed', e.detail)} />
							{/if}
						{:else}
							<div class="sbody static">
								<span class="srole">{COLUMNS[columnOf(s.type)]}</span>
								<span class="sname">{s.name}</span>
								{#if s.detail}<span class="sdetail">{s.detail}</span>{/if}
							</div>
						{/if}
					</div>
				</div>
			{:else}
				<p class="muted">No steps yet. Edit the route to describe the path.</p>
			{/each}
			{#if notice}<p class="notice">✓ {notice}</p>{/if}
			{#if canMerge}
				<div class="vacts"><button class="vadd" on:click={() => dispatch('merge')}>⤵ Merge a chain in as an option</button></div>
			{/if}
		</section>

		<section aria-label="Where it lives">
			<div class="kicker">Where it lives</div>
			<div class="rchips" role="group" aria-label="Rooms {chain.name} is in">
				{#each rooms as r (r.id)}
					<button class="rchip" class:on={inRooms.includes(r.id)} aria-pressed={inRooms.includes(r.id)}
						disabled={movingRooms} on:click={() => toggleRoom(r.id)}>
						<RoomIcon name={roomGlyph(r)} size={13} title="" />{r.name}
					</button>
				{/each}
				<a class="rmanage" href="/chains/rooms">+ Room</a>
			</div>
			<p class="muted rnote">
				{#if inRooms.length > 1}
					In {inRooms.length} rooms. Gear is drawn once on the graph, in the room it gets the most hours in.
				{:else if !inRooms.length}
					In no room yet. Rooms only group chains on this page; where plays land is unchanged.
				{:else}
					Rooms only group chains on this page; where plays land is unchanged.
				{/if}
			</p>
		</section>

		{#if older.length}
			<section aria-label="Kept from before slots">
				<div class="kicker">Kept from before</div>
				<p class="muted">
					Named before slots existed. These add or remove a box rather than swap one, so they can’t be a pick.
					Their hours stay on the gear they ran through.
				</p>
				{#each older as v (v.id)}
					<div class="vrow">
						<span class="vname">{v.name}</span>
						<span class="vend">
							<span class="vhrs">{formatHours(v.total_hours)}</span>
							<button class="ibtn danger" title="Remove (its plays stay on this chain, as wired)" aria-label="Remove {v.name}"
								on:click={() => dispatch('removeVariant', v)}>✕</button>
						</span>
					</div>
				{/each}
			</section>
		{/if}

		<section aria-label="How plays find this chain">
			<div class="kicker">How plays find this chain</div>
			{#each routing as r (r.key)}
				<div class="rule">
					<span class="tag tag-neutral rtag" title={r.kind}>{r.label}</span>
					<span class="rsays">{r.says}</span>
				</div>
			{:else}
				<p class="muted">
					Nothing sends plays here on its own yet. They land here when a client names this
					chain, or when an output or machine below the graph is set to it.
				</p>
			{/each}
		</section>

		{#if gearHours.length}
			<section aria-label="Gear hours on this chain">
				<div class="kicker">Gear hours on this chain</div>
				<div class="gtiles">
					{#each gearHours as g (g.name)}
						<div class="gtile"><div class="gbig">{formatHours(g.hours)}</div><div class="gname">{g.name}</div></div>
					{/each}
				</div>
			</section>
		{/if}

		<section class="who" aria-label="Who sees these plays">
			<div class="wtext">
				<div class="wtitle">Who sees these plays</div>
				<p class="muted">
					{#if chain.shared}
						Decks you’ve patched in see these plays and the format they arrived in. Never this
						chain’s name, gear or hours.
					{:else}
						Nobody but you.
					{/if}
				</p>
			</div>
			<div class="seg" role="group" aria-label="Who sees plays through {chain.name}">
				<button class="seg-btn" class:on={!chain.shared} aria-pressed={!chain.shared} disabled={sharing}
					on:click={() => chain.shared && dispatch('share', false)}>Only me</button>
				<button class="seg-btn" class:on={chain.shared} aria-pressed={chain.shared} disabled={sharing}
					on:click={() => !chain.shared && dispatch('share', true)}>Friends too</button>
			</div>
		</section>
	</div>

	<footer class="dfoot">
		<button class="btn btn-ghost del" on:click={() => dispatch('delete')}>Delete chain</button>
		<span style="flex:1"></span>
		<button class="btn btn-secondary" on:click={() => dispatch('duplicate')}>Duplicate</button>
		<button class="btn btn-primary" on:click={() => dispatch('edit')}>Edit the route</button>
	</footer>
</aside>

<style>
	.back {
		position: fixed;
		inset: 0;
		z-index: 80;
		background: rgba(27, 23, 18, 0.42);
		backdrop-filter: blur(2px);
		animation: td-fade-in 0.2s ease;
	}
	.drawer {
		position: fixed;
		top: 0;
		right: 0;
		z-index: 81;
		width: min(520px, 100%);
		height: 100vh;
		background: var(--td-surface);
		border-left: 1px solid var(--td-border);
		box-shadow: -14px 0 44px rgba(46, 43, 37, 0.22);
		display: flex;
		flex-direction: column;
		animation: td-slide-in 0.28s cubic-bezier(0.4, 0, 0.2, 1);
	}
	@media (prefers-reduced-motion: reduce) {
		.drawer,
		.back {
			animation: none;
		}
	}
	.dhead {
		display: flex;
		align-items: flex-start;
		gap: 12px;
		padding: 24px 24px 16px;
	}
	.dicon {
		flex: none;
		width: 44px;
		height: 44px;
		border-radius: 999px;
		display: flex;
		align-items: center;
		justify-content: center;
		background: var(--td-accent);
		color: var(--td-on-accent);
	}
	.dtitle {
		flex: 1;
		min-width: 0;
	}
	.dtitle h2 {
		font-size: 26px;
		line-height: 1.1;
	}
	.dsub {
		margin: 4px 0 0;
		font-size: 13px;
		color: var(--td-muted);
	}
	.ddesc {
		margin: 6px 0 0;
		font-size: 13px;
		color: var(--td-muted-2);
	}
	.dctx {
		margin-top: 8px;
	}
	.dclose {
		width: 40px;
		padding: 0;
		justify-content: center;
	}
	.dbody {
		flex: 1;
		overflow-y: auto;
		padding: 4px 24px 24px;
		display: flex;
		flex-direction: column;
		gap: 24px;
	}
	.kicker {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin-bottom: 10px;
	}
	.muted {
		margin: 0;
		font-size: 13px;
		color: var(--td-muted);
		line-height: 1.45;
	}
	.step {
		display: flex;
		gap: 12px;
		align-items: stretch;
	}
	.rail {
		display: flex;
		flex-direction: column;
		align-items: center;
		flex: none;
		width: 16px;
	}
	.rdot {
		width: 14px;
		height: 14px;
		border-radius: 999px;
		margin-top: 12px;
		background: var(--td-accent);
		box-shadow: 0 0 0 4px color-mix(in srgb, var(--td-accent) 16%, transparent);
	}
	.rline {
		width: 2px;
		flex: 1;
		min-height: 18px;
		margin: 3px 0;
		background: color-mix(in srgb, var(--td-muted-3) 55%, transparent);
	}
	.sbody {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 1px;
		text-align: left;
		border: none;
		background: transparent;
		font: inherit;
		color: inherit;
		padding: 8px 10px;
		margin-bottom: 6px;
		border-radius: 12px;
		cursor: pointer;
	}
	.sbox {
		flex: 1;
		min-width: 0;
		padding-bottom: 6px;
	}
	.sbox .sbody {
		width: 100%;
		margin-bottom: 0;
	}
	.sbody.static {
		cursor: default;
	}
	.pill {
		display: inline-flex;
		align-items: center;
		padding: 1px 10px;
		border-radius: 999px;
		border: 1.5px dashed var(--td-accent);
		color: var(--td-text);
	}
	.notice {
		margin: 4px 0 0;
		font-size: 13px;
		color: var(--td-good-fg);
	}
	.sbody:hover,
	.sbody:focus-visible {
		background: var(--td-surface2);
	}
	.srole {
		font-size: 11.5px;
		font-weight: 600;
		color: var(--td-muted-2);
	}
	.sname {
		font-size: 15.5px;
		font-weight: 700;
		display: flex;
		align-items: center;
		gap: 8px;
	}
	.shint {
		font-size: 10.5px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 500;
		color: var(--td-muted-3);
		opacity: 0;
	}
	.sbody:hover .shint,
	.sbody:focus-visible .shint {
		opacity: 1;
	}
	.sdetail {
		font-size: 12.5px;
		color: var(--td-muted-2);
	}
	.rchips {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		align-items: center;
	}
	.rchip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		min-height: 34px;
		padding: 0 12px;
		border-radius: 999px;
		border: 1px solid var(--td-border);
		background: var(--td-surface2);
		color: var(--td-text);
		font: inherit;
		font-size: 13px;
		font-weight: 600;
		cursor: pointer;
	}
	.rchip.on {
		background: var(--td-accent);
		border-color: var(--td-accent);
		color: var(--td-on-accent);
	}
	.rchip:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.rmanage {
		font-size: 13px;
		color: var(--td-accent);
		padding: 0 6px;
	}
	.rnote {
		margin-top: 8px;
		font-size: 12.5px;
	}
	.vrow {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 12px;
		padding: 6px 8px 6px 14px;
		margin-bottom: 7px;
	}
	.vname {
		font-size: 14px;
		min-width: 0;
	}
	.vend {
		display: flex;
		align-items: center;
		gap: 4px;
		flex: none;
	}
	.vhrs {
		font-size: 12.5px;
		color: var(--td-muted-2);
		font-variant-numeric: tabular-nums;
		margin-right: 4px;
	}
	.ibtn {
		border: none;
		background: transparent;
		cursor: pointer;
		color: var(--td-muted-2);
		width: 26px;
		height: 26px;
		border-radius: 8px;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		font: inherit;
	}
	.ibtn:hover {
		background: var(--td-nav-hover);
		color: var(--td-text);
	}
	.ibtn.danger:hover {
		color: var(--td-bad-fg);
	}
	.vacts {
		display: flex;
		gap: 8px;
		flex-wrap: wrap;
		margin-top: 4px;
	}
	.vadd {
		background: none;
		border: 1px dashed var(--td-border);
		border-radius: 999px;
		padding: 4px 12px;
		font-size: 12.5px;
		color: var(--td-muted);
		cursor: pointer;
		font-family: inherit;
	}
	.vadd:hover {
		border-color: var(--td-accent);
		color: var(--td-text);
	}
	.rule {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 8px 0;
		border-top: 1px solid var(--td-border);
	}
	.rtag {
		min-width: 118px;
		justify-content: center;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 180px;
	}
	.rsays {
		flex: 1;
		font-size: 13.5px;
	}
	.gtiles {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: 8px;
	}
	.gtile {
		background: var(--td-surface2);
		border: 1px solid var(--td-border);
		border-radius: 14px;
		padding: 12px 14px;
	}
	.gbig {
		font-family: 'Caprasimo', serif;
		font-size: 22px;
		line-height: 1;
	}
	.gname {
		font-size: 12px;
		color: var(--td-muted);
		margin-top: 4px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.who {
		display: flex;
		align-items: center;
		gap: 12px;
		flex-wrap: wrap;
		padding: 16px 18px;
		border-radius: 20px;
		background: var(--td-bg);
		border: 1px solid var(--td-border);
	}
	.wtext {
		flex: 1 1 240px;
		min-width: 0;
	}
	.wtitle {
		font-weight: 700;
		margin-bottom: 2px;
	}
	.seg {
		display: flex;
		gap: 4px;
		background: var(--td-sidebar);
		border-radius: 999px;
		padding: 4px;
	}
	.seg-btn {
		white-space: nowrap;
		border: none;
		cursor: pointer;
		font-family: inherit;
		font-size: 12.5px;
		padding: 8px 14px;
		border-radius: 999px;
		background: transparent;
		color: var(--td-nav-fg);
		font-weight: 500;
	}
	.seg-btn.on {
		background: var(--td-accent);
		color: var(--td-on-accent);
		font-weight: 600;
	}
	.seg-btn:disabled {
		opacity: 0.6;
		cursor: default;
	}
	.dfoot {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 14px 24px 20px;
		border-top: 1px solid var(--td-border);
		flex-wrap: wrap;
	}
	.del {
		color: var(--td-bad-fg);
	}
</style>
