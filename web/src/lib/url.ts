import { goto } from '$app/navigation';

/**
 * Rewrite the address bar to match what's on screen, without navigating.
 *
 * For the pages that keep their state in the URL so a view survives a reload —
 * the chart you're looking at, a search, a report period, the history's scope.
 *
 * It must **not** be `history.replaceState(null, …)`, which is the obvious way
 * to write this and breaks the Back button in a way nothing reports. SvelteKit
 * keeps its own history index inside `history.state`, and passing `null` wipes
 * it off that entry. The router's `popstate` handler then takes the "not ours"
 * branch and only updates `page.url`: the address bar changes and the rendered
 * page does not. So the first Back looks like it did nothing, and the second
 * jumps *two* entries, because the index it counts a delta from was never
 * moved by the pop it ignored. Reported as "back doesn't work until I click it
 * twice, and then it goes two pages back".
 */
export function syncUrl(url: string): void {
	void goto(url, { replaceState: true, noScroll: true, keepFocus: true });
}
