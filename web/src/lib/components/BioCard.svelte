<script lang="ts">
	import type { BioBlock } from '$lib/api';

	export let bio: BioBlock;
	export let heading = 'About this release';

	/** Clamped to a few lines until asked to expand — client-only state, no
	 *  refetch. */
	let expanded = false;

	// Rendered as text, never {@html}: the providers return prose wrapped in
	// their own markup, which is stripped server-side, and treating third-party
	// text as HTML would be an injection waiting to happen.
	$: text = bio?.text ?? null;
	$: long = (text?.length ?? 0) > 320;
</script>

<div class="bio">
	<div class="head">
		<svg viewBox="0 0 24 24" width="16" fill="none" stroke="#c67139" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 5h16"/><path d="M4 10h16"/><path d="M4 15h10"/></svg>
		<span>{heading}</span>
	</div>

	{#if text}
		<p class="body" class:clamped={long && !expanded}>{text}</p>
		<div class="foot">
			{#if long}
				<button class="btn btn-ghost more" on:click={() => (expanded = !expanded)}>
					{expanded ? 'Show less' : 'Read more'}
				</button>
			{:else}
				<span></span>
			{/if}
			<!-- Not decoration: Last.fm's text is CC BY-SA, so the credit has to
			     travel with the prose wherever it is shown. -->
			{#if bio.attribution}
				<span class="credit">
					{#if bio.url}
						<a href={bio.url} target="_blank" rel="noopener nofollow">{bio.attribution}</a>
					{:else}
						{bio.attribution}
					{/if}
				</span>
			{/if}
		</div>
	{:else}
		<p class="empty">
			No description on file — none of Last.fm, TheAudioDB, Apple Music or MusicBrainz had one.
		</p>
	{/if}
</div>

<style>
	.bio {
		background: var(--td-surface);
		border: 1px solid var(--td-border);
		border-radius: 22px;
		padding: 22px 24px;
		margin-bottom: 30px;
	}
	.head {
		display: flex;
		align-items: center;
		gap: 8px;
		margin-bottom: 12px;
		font-size: 11px;
		letter-spacing: .13em;
		text-transform: uppercase;
		color: var(--td-muted-2);
		font-weight: 600;
	}
	.body {
		margin: 0;
		font-size: 14.5px;
		line-height: 1.66;
		color: var(--td-nav-fg);
		white-space: pre-wrap;
	}
	.clamped {
		display: -webkit-box;
		-webkit-line-clamp: 4;
		line-clamp: 4;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}
	.foot {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		margin-top: 14px;
		flex-wrap: wrap;
	}
	.more { font-size: 12.5px; padding: 5px 12px; white-space: nowrap; }
	.credit { font-size: 11.5px; color: var(--td-muted-3); }
	.empty { margin: 0; font-size: 13.5px; color: var(--td-muted-2); }
</style>
