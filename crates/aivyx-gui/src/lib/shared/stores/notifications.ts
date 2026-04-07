// ── Notifications Store ────────────────────────────────────────
// Subscribes to GET /api/notifications SSE stream and maintains a
// live ring buffer of the most recent notifications.

import { writable, derived } from 'svelte/store';
import { subscribeNotifications } from '$lib/shared/api/client';
import type { Notification } from '$lib/shared/api/types';

const MAX_NOTIFICATIONS = 200;

export const notifications = writable<Notification[]>([]);
export const latestNotification = writable<Notification | null>(null);
export const notificationConnected = writable(false);

// Unread count (requires_approval or recent)
export const pendingApprovalNotifications = derived(
	notifications,
	($ns) => $ns.filter((n) => n.requires_approval).length,
);

export function sourceColor(source: string): string {
	if (source.includes('heartbeat')) return 'var(--color-sage)';
	if (source === 'schedule' || source === 'briefing') return 'var(--color-secondary)';
	if (source === 'triage' || source === 'email') return 'var(--color-warning)';
	if (source === 'goal') return 'var(--color-accent-glow)';
	if (source === 'mission') return 'var(--color-primary)';
	return 'var(--color-on-surface-variant)';
}

export function sourceIcon(source: string): string {
	if (source.includes('heartbeat')) return 'favorite';
	if (source === 'schedule' || source === 'briefing') return 'schedule';
	if (source === 'triage' || source === 'email') return 'mail';
	if (source === 'goal') return 'flag';
	if (source === 'mission') return 'rocket_launch';
	return 'notifications';
}

let cleanup: (() => void) | null = null;

/** Start the SSE notification stream. Call once from the root layout. */
export function startNotificationStream(): () => void {
	notificationConnected.set(false);

	cleanup = subscribeNotifications(
		(notif) => {
			notificationConnected.set(true);
			latestNotification.set(notif);
			notifications.update((ns) => {
				const updated = [notif, ...ns];
				return updated.slice(0, MAX_NOTIFICATIONS);
			});
		},
		() => {
			notificationConnected.set(false);
			// Reconnect after 5s on error
			setTimeout(startNotificationStream, 5_000);
		},
	);

	return () => {
		cleanup?.();
		cleanup = null;
	};
}
