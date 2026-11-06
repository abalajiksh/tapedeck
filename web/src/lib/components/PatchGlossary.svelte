<script lang="ts">
	// The words Patch uses, and what the same thing is called elsewhere.
	//
	// Tapedeck keeps its own names on screen (Patch, Reel, Spool, Dub, Love) and
	// speaks the common vocabulary on the wire: ActivityPub where there is a
	// word for it, and Rocksky's names alongside for anyone arriving from there
	// (Ashwin, 2026-11-06). This is the one place that lines them up.
	//
	// Static on purpose: nothing here comes from the server, so it cannot drift
	// with an instance's state. When a term changes, change it here too.

	type Term = {
		word: string;
		means: string;
		rocksky: string | null;
		rockskyId?: string;
		rockskyNote?: string;
		ap: string | null;
		apNote?: string;
	};

	type Group = { title: string; terms: Term[] };

	const GROUPS: Group[] = [
		{
			title: 'People',
			terms: [
				{
					word: 'Deck',
					means: 'A person here, and the history they have chosen to let others see.',
					rocksky: 'Profile',
					rockskyId: 'app.bsky.actor.profile',
					ap: 'Actor',
					apNote: 'a Person, found by its handle'
				},
				{
					word: 'Handle',
					means: 'A deck’s address, @name@server, once its owner switches federation on.',
					rocksky: 'Handle',
					ap: 'WebFinger acct:',
					apNote: 'how another server looks a deck up'
				},
				{
					word: 'Patch',
					means:
						'Asking to hear another deck. It is a request: nothing reaches you until they accept.',
					rocksky: 'Follow',
					rockskyId: 'app.rocksky.graph.follow',
					ap: 'Follow',
					apNote: 'answered with Accept or Reject'
				},
				{
					word: 'Unpatch',
					means: 'Stop hearing a deck. Either side can end it.',
					rocksky: 'Unfollow',
					ap: 'Undo',
					apNote: 'of the Follow'
				},
				{
					word: 'Decks you’ve patched into',
					means: 'The decks whose listening reaches your Reel.',
					rocksky: 'Following',
					ap: 'following',
					apNote: 'collection'
				},
				{
					word: 'Decks you’ve patched in',
					means: 'The people you let hear your deck.',
					rocksky: 'Followers',
					ap: 'followers',
					apNote: 'collection'
				},
				{
					word: 'Instance',
					means: 'This Tapedeck server and everyone on it.',
					rocksky: 'AppView',
					rockskyNote: 'where records are indexed; each person’s live in their own PDS',
					ap: 'Server'
				}
			]
		},
		{
			title: 'Listening',
			terms: [
				{
					word: 'Listen',
					means: 'One play of one track. Skips are kept for you and never shown to anyone.',
					rocksky: 'Scrobble',
					rockskyId: 'app.rocksky.scrobble',
					ap: 'Create',
					apNote: 'wrapping a td:Listen'
				},
				{
					word: 'The Reel',
					means: 'Your timeline: what the decks you’ve patched into have been playing.',
					rocksky: 'Feed',
					rockskyId: 'app.rocksky.feed.generator',
					ap: 'Home timeline',
					apNote: 'built from the Create activities you receive'
				},
				{
					word: 'Live',
					means: 'Who is playing something right now. Held in memory, never stored.',
					rocksky: 'Status',
					rockskyId: 'app.rocksky.actor.status',
					ap: null
				},
				{
					word: 'Love',
					means: 'Marking a track, album or artist you love, into your own loves.',
					rocksky: 'Like',
					rockskyId: 'app.rocksky.like',
					ap: 'Like'
				},
				{
					word: 'Delete',
					means: 'Removing a listen. It is withdrawn wherever it went.',
					rocksky: 'Deleting the record',
					ap: 'Delete',
					apNote: 'leaving a Tombstone'
				}
			]
		},
		{
			title: 'Together',
			terms: [
				{
					word: 'Shared Spool',
					means:
						'Listening together: one deck plays, the others in the room can take the plays into their own history.',
					rocksky: null,
					ap: null
				},
				{
					word: 'Sitting',
					means: 'One stretch of listening, start to finish. You can hand a past one to somebody.',
					rocksky: null,
					ap: null
				},
				{
					word: 'Dub',
					means: 'Passing somebody a record, with a note.',
					rocksky: null,
					ap: null
				},
				{
					word: 'Liner notes',
					means: 'Your own writing about a track, album, artist or gear.',
					rocksky: 'Shout (nearest)',
					rockskyId: 'app.rocksky.shout',
					rockskyNote: 'a shout is said to someone; liner notes are yours',
					ap: null
				}
			]
		},
		{
			title: 'Your gear',
			terms: [
				{
					word: 'Signal chain',
					means:
						'The path from source to ears. Private by design: nobody else is ever told its name, gear or hours.',
					rocksky: null,
					ap: null,
					apNote: 'never sent'
				},
				{
					word: 'Shared chain',
					means:
						'A chain whose listens others may see, with the format each one arrived in: codec, rate, depth, and what it was transcoded to.',
					rocksky: null,
					ap: 'td: format fields',
					apNote: 'on the listen, never the chain'
				}
			]
		}
	];

	let q = '';
	$: needle = q.trim().toLowerCase();
	$: shown = GROUPS.map((g) => ({
		title: g.title,
		terms: needle
			? g.terms.filter((t) =>
					[t.word, t.means, t.rocksky, t.rockskyId, t.rockskyNote, t.ap, t.apNote]
						.filter(Boolean)
						.some((x) => (x as string).toLowerCase().includes(needle))
				)
			: g.terms
	})).filter((g) => g.terms.length);
</script>

<div class="panel gloss">
	<div class="ghead">
		<div>
			<h2 class="h-display" style="font-size:22px;margin:0 0 4px;">Glossary</h2>
			<p class="gsub">
				Tapedeck uses its own words on screen. Between servers it speaks ActivityPub, and the
				Rocksky column is there for anyone coming from Rocksky. Tapedeck does not deliver to
				other servers yet; the ActivityPub column is what it will send when it does.
			</p>
		</div>
		<input
			class="input gsearch"
			type="search"
			placeholder="Find a word…"
			aria-label="Find a word in the glossary"
			bind:value={q}
		/>
	</div>

	<div class="grow gcols" aria-hidden="true">
		<span>Here</span><span>What it means</span><span>Rocksky</span><span>ActivityPub</span>
	</div>

	{#each shown as g (g.title)}
		<h3 class="gtitle">{g.title}</h3>
		<dl class="glist">
			{#each g.terms as t (t.word)}
				<div class="grow">
					<dt class="gword">{t.word}</dt>
					<dd class="gmeans">{t.means}</dd>
					<dd class="gother">
						<span class="glabel">Rocksky</span>
						{#if t.rocksky}
							{t.rocksky}
							<!-- Breaks only at the dots, never mid-word. -->
							{#if t.rockskyId}<code
									>{#each t.rockskyId.split('.') as part, i}{i ? '.' : ''}<wbr />{part}{/each}</code
								>{/if}
						{:else}
							<span class="gnone">No equivalent</span>
						{/if}
						{#if t.rockskyNote}<span class="gnote">{t.rockskyNote}</span>{/if}
					</dd>
					<dd class="gother">
						<span class="glabel">ActivityPub</span>
						{#if t.ap}
							{t.ap}
						{:else}
							<span class="gnone">Not sent</span>
						{/if}
						{#if t.apNote}<span class="gnote">{t.apNote}</span>{/if}
					</dd>
				</div>
			{/each}
		</dl>
	{:else}
		<p class="gsub">No word here matches “{q}”.</p>
	{/each}
</div>

<style>
	.gloss {
		border-radius: 22px;
		padding: 22px 24px;
		/* Stacks on the panel's own width, not the window's: the sidebar takes
		   a different share of the screen at every size. */
		container-type: inline-size;
	}
	.ghead {
		display: flex;
		gap: 16px;
		align-items: flex-start;
		justify-content: space-between;
		flex-wrap: wrap;
		margin-bottom: 14px;
	}
	.gsub {
		margin: 0;
		font-size: 12.5px;
		color: var(--td-muted-2);
		max-width: 64ch;
		line-height: 1.5;
	}
	.gsearch {
		width: 220px;
		max-width: 100%;
	}
	.gtitle {
		font-size: 11px;
		letter-spacing: 0.1em;
		text-transform: uppercase;
		font-weight: 700;
		color: var(--td-muted-2);
		margin: 18px 0 6px;
	}
	.glist {
		margin: 0;
	}
	.grow {
		display: grid;
		grid-template-columns: minmax(7rem, 0.7fr) minmax(12rem, 2fr) minmax(8rem, 1fr) minmax(8rem, 1fr);
		gap: 14px;
		padding: 10px 0;
		border-top: 1px solid var(--td-border);
		align-items: baseline;
	}
	.gcols {
		border-top: none;
		padding: 0 0 4px;
		font-size: 11px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--td-muted-3);
	}
	.gword {
		font-weight: 600;
		font-size: 13.5px;
	}
	.gmeans {
		margin: 0;
		font-size: 13px;
		color: var(--td-muted);
		line-height: 1.45;
	}
	.gother {
		margin: 0;
		font-size: 13px;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.gother code {
		font: 500 11px ui-monospace, SFMono-Regular, Menlo, monospace;
		color: var(--td-muted-3);
		overflow-wrap: break-word;
	}
	.gnote {
		font-size: 11.5px;
		color: var(--td-muted-3);
	}
	.gnone {
		font-style: italic;
		color: var(--td-muted-3);
	}
	.glabel {
		display: none;
		font-size: 10.5px;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--td-muted-3);
	}
	@container (max-width: 640px) {
		.grow {
			grid-template-columns: 1fr;
			gap: 4px;
		}
		.gcols {
			display: none;
		}
		.glabel {
			display: block;
			margin-top: 6px;
		}
	}
</style>
