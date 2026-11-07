// The variant an output (or anything else) points at to put one piece of gear
// on by itself at a chain's slot ("Bluetooth puts the TW4 on").
//
// An existing variant that says exactly that is reused; otherwise one is made
// through the variants API and Tapedeck takes it as a pick of its own on the
// next read (`adopt_setups`), so its name is never shown.

import { api, type ChainComponent, type ChainVariant } from '$lib/api';

/** The slot a piece of gear can be on at, and whether it is the wired gear. */
export function slotFor(components: ChainComponent[], gear: number): { position: number; wired: boolean } | null {
	const i = components.findIndex((c) => (c.options?.length ?? 0) >= 2 && c.options!.includes(gear));
	return i < 0 ? null : { position: i, wired: components[i].equipment_id === gear };
}

/** The variant of `chainId` that puts `gear` on by itself, made if needed.
 *  Null when the gear is what the slot is wired with. */
export async function pickVariant(
	chainId: number,
	components: ChainComponent[],
	variants: ChainVariant[],
	gear: number,
	gearName: string
): Promise<number | null> {
	const slot = slotFor(components, gear);
	if (!slot) throw new Error(`${gearName} is not an option at any step of this chain`);
	if (slot.wired) return null;
	const have = variants.find(
		(v) =>
			v.chain_id === chainId &&
			v.overrides.length === 1 &&
			v.overrides[0].position === slot.position &&
			v.overrides[0].equipment_id === gear
	);
	if (have) return have.id;
	return createPick(chainId, slot.position, gear, gearName, variants);
}

/** Make the variant for one pick, named after the gear like the ones the
 *  server makes (some screens print the name as it is), with a counter when
 *  the chain already has a variant of that name. */
export async function createPick(
	chainId: number,
	position: number,
	gear: number,
	gearName: string,
	variants: ChainVariant[]
): Promise<number> {
	const taken = new Set(variants.filter((v) => v.chain_id === chainId).map((v) => v.name));
	let name = gearName;
	for (let n = 2; taken.has(name); n++) name = `${gearName} (${n})`;
	const made = await api.createVariant({
		chain_id: chainId,
		name,
		overrides: [{ position, equipment_id: gear, name: gearName }]
	});
	return made.id;
}
