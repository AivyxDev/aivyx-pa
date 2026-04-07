// ── Dashboard Store ────────────────────────────────────────────
// Polls GET /api/dashboard every 30s and exposes reactive agent state.

import { writable, derived } from 'svelte/store';
import { getDashboard } from '$lib/shared/api/client';
import type { DashboardResponse } from '$lib/shared/api/types';

export const dashboard = writable<DashboardResponse | null>(null);
export const dashboardError = writable<string | null>(null);
export const dashboardLoading = writable(true);

// Derived convenience values used throughout the layout
export const agentName = derived(dashboard, ($d) => $d?.agent_name ?? 'Aivyx');
export const pendingApprovals = derived(dashboard, ($d) => $d?.approvals_pending ?? 0);
export const activeGoals = derived(dashboard, ($d) => $d?.goals_active ?? 0);
export const activeMissions = derived(dashboard, ($d) => $d?.missions_active ?? 0);
export const heartbeatActive = derived(dashboard, ($d) => $d?.heartbeat_enabled ?? false);

let pollInterval: ReturnType<typeof setInterval> | null = null;

export async function refreshDashboard(): Promise<void> {
	try {
		const data = await getDashboard();
		dashboard.set(data);
		dashboardError.set(null);
	} catch (e) {
		dashboardError.set((e as Error).message);
	} finally {
		dashboardLoading.set(false);
	}
}

/** Start polling. Call once from the root layout. */
export function startDashboardPolling(intervalMs = 30_000): () => void {
	refreshDashboard();
	pollInterval = setInterval(refreshDashboard, intervalMs);
	return () => {
		if (pollInterval !== null) {
			clearInterval(pollInterval);
			pollInterval = null;
		}
	};
}
