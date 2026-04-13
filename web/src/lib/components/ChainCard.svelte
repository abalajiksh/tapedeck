<script lang="ts">
	import type { SignalChain } from '$lib/api';
	import { formatHours } from '$lib/utils';

	export let chain: SignalChain;

	const contextColors: Record<string, string> = {
		active:         'bg-rp-pine/20 text-rp-pine',
		'active-mobile':'bg-rp-foam/20 text-rp-foam',
		passive:        'bg-rp-gold/20 text-rp-gold',
		background:     'bg-rp-muted/20 text-rp-muted',
		unknown:        'bg-rp-hl-med text-rp-subtle',
	};

	const typeIcons: Record<string, string> = {
		source: '💻',
		dac: '🎛',
		amp: '🔊',
		transducer: '🎧',
		transport: '📡',
		network: '🌐',
		bluetooth: '📶',
	};
</script>

<div class="rounded-lg bg-rp-surface border border-rp-hl-med p-4">
	<div class="flex items-start justify-between mb-3">
		<div>
			<h3 class="text-sm font-medium text-rp-text">{chain.name}</h3>
			{#if chain.description}
				<p class="text-xs text-rp-muted mt-0.5">{chain.description}</p>
			{/if}
		</div>
		<span class="text-[10px] px-2 py-0.5 rounded-full uppercase tracking-wider font-medium
		             {contextColors[chain.listening_context] ?? contextColors.unknown}">
			{chain.listening_context}
		</span>
	</div>

	<!-- Chain flow -->
	<div class="flex items-center gap-1 flex-wrap">
		{#each chain.components as comp, i}
			<div class="flex items-center gap-1 text-xs">
				<span class="inline-flex items-center gap-1 px-2 py-1 rounded bg-rp-hl-low text-rp-subtle">
					<span>{typeIcons[comp.type] ?? '•'}</span>
					{comp.name}
				</span>
				{#if i < chain.components.length - 1}
					<span class="text-rp-hl-high">→</span>
				{/if}
			</div>
		{/each}
	</div>

	<!-- Hours -->
	<p class="text-xs text-rp-muted mt-3">{formatHours(chain.total_hours)} total</p>
</div>
