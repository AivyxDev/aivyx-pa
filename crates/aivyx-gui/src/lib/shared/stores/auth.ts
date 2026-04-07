import { writable, derived } from 'svelte/store';

export type AppState = 'uninitialized' | 'locked' | 'unlocked';

function createAuthStore() {
	const { subscribe, set } = writable<AppState>('locked');

	return {
		subscribe,
		unlock: () => set('unlocked'),
		lock: () => set('locked'),
		setUninitialized: () => set('uninitialized'),
		set
	};
}

export const authState = createAuthStore();
export const isUnlocked = derived(authState, ($state) => $state === 'unlocked');
export const isLocked = derived(authState, ($state) => $state === 'locked');
export const isUninitialized = derived(authState, ($state) => $state === 'uninitialized');
