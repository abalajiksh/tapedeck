// Gear types, chain roles and the guesses between them, shared by the route
// editor and the new-chain wizard. The chains page keeps its own copies for
// its dialogs; values must stay in step with `ComponentType` in src/models.rs.

/** A chain step's role. Serde refuses anything else with a 422. */
export const COMP_TYPES = [
	{ v: 'source', label: 'Plays from' },
	{ v: 'transport', label: 'Transport' },
	{ v: 'dac', label: 'DAC' },
	{ v: 'amp', label: 'Amplifier' },
	{ v: 'transducer', label: 'Headphones / speakers' },
	{ v: 'network', label: 'Network' },
	{ v: 'bluetooth', label: 'Bluetooth' }
];
export const compTypeLabel = (v: string) => COMP_TYPES.find((t) => t.v === v)?.label ?? v;

/** The role a piece of gear usually plays, guessed once when it is placed. */
export const COMPONENT_TYPE_FOR_GEAR: Record<string, string> = {
	headphones: 'transducer', iem: 'transducer', speakers: 'transducer', subwoofer: 'transducer',
	'builtin-speakers': 'transducer', earbuds: 'bluetooth',
	dac: 'dac', 'dac-amp': 'dac', interface: 'dac', receiver: 'dac',
	amp: 'amp', preamp: 'amp', phono: 'amp',
	source: 'source', turntable: 'source', phone: 'source', radio: 'source',
	cartridge: 'source', headshell: 'source', computer: 'source', dap: 'source', other: 'source',
	transport: 'transport', cable: 'transport'
};
export const roleOfGear = (gearType: string) => COMPONENT_TYPE_FOR_GEAR[gearType] ?? 'source';

/** Where gear sits in the palette and the wizard. */
export type GearGroup = 'heard' | 'between' | 'from';
export function groupOf(gearType: string): GearGroup {
	const role = roleOfGear(gearType);
	if (role === 'transducer' || role === 'bluetooth') return 'heard';
	if (role === 'source') return 'from';
	return 'between';
}
export const GROUP_TITLE: Record<GearGroup, string> = {
	heard: 'Heard on',
	between: 'In between',
	from: 'Plays from'
};

/** Gear types for the inline "new gear" forms, by group. */
export const NEW_GEAR_TYPES: Record<GearGroup, { v: string; label: string }[]> = {
	heard: [
		{ v: 'headphones', label: 'Headphones' },
		{ v: 'iem', label: 'IEM' },
		{ v: 'earbuds', label: 'Wireless earbuds' },
		{ v: 'speakers', label: 'Speakers' },
		{ v: 'builtin-speakers', label: 'Built-in speakers' }
	],
	between: [
		{ v: 'dac', label: 'DAC' },
		{ v: 'amp', label: 'Amplifier' },
		{ v: 'dac-amp', label: 'DAC / Amp combo' },
		{ v: 'phono', label: 'Phono stage' },
		{ v: 'preamp', label: 'Preamp' }
	],
	from: [
		{ v: 'phone', label: 'Phone' },
		{ v: 'computer', label: 'Computer' },
		{ v: 'dap', label: 'Music player' },
		{ v: 'turntable', label: 'Turntable' },
		{ v: 'source', label: 'Streamer / other source' },
		{ v: 'radio', label: 'Radio' }
	]
};

/** A chain icon from the gear it plays from. */
export const CHAIN_ICON_FOR_SOURCE: Record<string, string> = {
	phone: 'phone', dap: 'dap', computer: 'laptop', turntable: 'turntable', radio: 'radio',
	cartridge: 'turntable', transport: 'cdplayer'
};
