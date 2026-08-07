<script lang="ts">
	/** "Record of the month" / "Track of the month" — an icon block, a label and
	 *  two lines. Clickable when there is somewhere to go; a track has no page
	 *  of its own, so it opens the record it sits on, per the house rule. */
	export let kind: 'album' | 'track';
	export let label: string;
	export let title: string;
	export let sub: string;
	export let onOpen: (() => void) | null = null;
	export let iconSize = 64;
</script>

<svelte:element
	this={onOpen ? 'button' : 'div'}
	class="card"
	class:clickable={!!onOpen}
	role={onOpen ? 'button' : undefined}
	on:click={onOpen}
>
	<span class="icon" class:track={kind === 'track'} style="width:{iconSize}px;height:{iconSize}px;">
		{#if kind === 'album'}
			<svg viewBox="0 0 24 24" width={iconSize * 0.4} fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="3" width="14" height="14" rx="2"/><path d="M17 8h4v11a2 2 0 0 1-2 2H8"/><circle cx="10" cy="10" r="2.4"/></svg>
		{:else}
			<svg viewBox="0 0 24 24" width={iconSize * 0.4} fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l10-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="16" cy="16" r="3"/></svg>
		{/if}
	</span>
	<span class="body">
		<span class="k">{label}</span>
		<span class="t">{title}</span>
		<span class="s">{sub}</span>
	</span>
</svelte:element>

<style>
	.card {
		flex: 1; background: var(--td-track); border: none; border-radius: 18px;
		padding: 16px; display: flex; gap: 14px; align-items: center;
		font: inherit; color: inherit; text-align: left; min-width: 0; width: 100%;
	}
	.clickable { cursor: pointer; }
	.clickable:hover .t { color: var(--td-accent); }
	.icon {
		flex: none; border-radius: 12px; display: flex;
		align-items: center; justify-content: center;
		color: rgba(255, 255, 255, 0.85);
		background: linear-gradient(135deg, #8a5a2f, #c67139);
	}
	.icon.track { background: linear-gradient(135deg, #5f6f47, #7a8a5e); }
	.body { min-width: 0; }
	.k {
		display: block; font-size: 11px; letter-spacing: 0.1em;
		text-transform: uppercase; color: var(--td-muted-2); margin-bottom: 4px;
	}
	.t {
		display: block; font-weight: 700; font-size: 15px;
		overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
	}
	.s {
		display: block; font-size: 12.5px; color: var(--td-muted-2);
		overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
	}
</style>
