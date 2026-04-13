/** @type {import('tailwindcss').Config} */
export default {
	content: ['./src/**/*.{html,js,svelte,ts}'],
	theme: {
		extend: {
			colors: {
				// Rosé Pine
				rp: {
					base: '#191724',
					surface: '#1f1d2e',
					overlay: '#26233a',
					muted: '#6e6a86',
					subtle: '#908caa',
					text: '#e0def4',
					love: '#eb6f92',
					gold: '#f6c177',
					rose: '#ebbcba',
					pine: '#31748f',
					foam: '#9ccfd8',
					iris: '#c4a7e7',
					'hl-low': '#21202e',
					'hl-med': '#403d52',
					'hl-high': '#524f67',
				}
			},
			fontFamily: {
				sans: ['"Roboto"', 'system-ui', 'sans-serif'],
			},
		}
	},
	plugins: []
};
