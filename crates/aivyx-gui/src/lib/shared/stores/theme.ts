import { writable } from 'svelte/store';
import { browser } from '$app/environment';

type Theme = 'dark' | 'light';

function createThemeStore() {
	const stored = browser ? (localStorage.getItem('aivyx-theme') as Theme) : null;
	const initial: Theme = stored ?? 'dark';

	const { subscribe, set, update } = writable<Theme>(initial);

	return {
		subscribe,
		toggle: () => {
			update((current) => {
				const next: Theme = current === 'dark' ? 'light' : 'dark';
				if (browser) {
					localStorage.setItem('aivyx-theme', next);
					document.documentElement.classList.remove('dark', 'light');
					document.documentElement.classList.add(next);
				}
				return next;
			});
		},
		set: (theme: Theme) => {
			if (browser) {
				localStorage.setItem('aivyx-theme', theme);
				document.documentElement.classList.remove('dark', 'light');
				document.documentElement.classList.add(theme);
			}
			set(theme);
		}
	};
}

export const theme = createThemeStore();
