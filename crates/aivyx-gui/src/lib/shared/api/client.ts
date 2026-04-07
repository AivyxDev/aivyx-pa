// ── Aivyx PA API Client ────────────────────────────────────────
// Typed HTTP + SSE client for the PA HTTP API (port 3100).
// All paths mirror api.rs exactly.

import type {
	DashboardResponse,
	HealthResponse,
	Goal,
	ApprovalItem,
	AuditEntry,
	AuditMetrics,
	MemoryEntry,
	Notification,
	ChatSession,
	ChatMessage,
	SettingsSnapshot,
} from './types';

export const API_BASE = 'http://127.0.0.1:3100';

// ── Generic fetch helper ───────────────────────────────────────

async function apiFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
	const res = await fetch(`${API_BASE}${path}`, {
		headers: { 'Content-Type': 'application/json', ...(init.headers ?? {}) },
		...init,
	});
	if (!res.ok) {
		const text = await res.text().catch(() => res.statusText);
		throw new Error(`API ${res.status}: ${text}`);
	}
	return res.json() as Promise<T>;
}

// ── Health ─────────────────────────────────────────────────────

export const getHealth = () =>
	apiFetch<HealthResponse>('/api/health');

// ── Dashboard ──────────────────────────────────────────────────

export const getDashboard = () =>
	apiFetch<DashboardResponse>('/api/dashboard');

// ── Chat ───────────────────────────────────────────────────────

export const getSessions = () =>
	apiFetch<{ sessions: ChatSession[] }>('/api/sessions');

export const createSession = (title?: string) =>
	apiFetch<{ session: ChatSession }>('/api/sessions', {
		method: 'POST',
		body: JSON.stringify({ title }),
	});

export const deleteSession = (id: string) =>
	apiFetch<void>(`/api/sessions/${id}`, { method: 'DELETE' });

export const getSessionMessages = (id: string) =>
	apiFetch<{ messages: [string, string][] }>(`/api/sessions/${id}/messages`);

/**
 * POST /api/chat — returns a ReadableStream of SSE events.
 * Each `token` event carries a text chunk; `done` carries { session_id }.
 */
export function streamChat(message: string, session_id?: string): EventSource {
	// We use fetch + ReadableStream rather than EventSource since we POST.
	// Callers handle the stream via the returned fetch Response.
	throw new Error('Use streamChatFetch instead');
}

export async function streamChatFetch(
	message: string,
	session_id: string | undefined,
	onToken: (chunk: string) => void,
	onDone: (session_id: string) => void,
	onError?: (err: Error) => void,
): Promise<void> {
	let res: Response;
	try {
		res = await fetch(`${API_BASE}/api/chat`, {
			method: 'POST',
			headers: { 'Content-Type': 'application/json' },
			body: JSON.stringify({ message, session_id }),
		});
	} catch (e) {
		onError?.(e as Error);
		return;
	}

	if (!res.ok || !res.body) {
		onError?.(new Error(`Chat API ${res.status}`));
		return;
	}

	const reader = res.body.getReader();
	const decoder = new TextDecoder();
	let buffer = '';

	while (true) {
		const { done, value } = await reader.read();
		if (done) break;
		buffer += decoder.decode(value, { stream: true });

		// Parse SSE lines
		const lines = buffer.split('\n');
		buffer = lines.pop() ?? '';

		let eventType = '';
		for (const line of lines) {
			if (line.startsWith('event: ')) {
				eventType = line.slice(7).trim();
			} else if (line.startsWith('data: ')) {
				const data = line.slice(6).trim();
				if (eventType === 'token') {
					onToken(data);
				} else if (eventType === 'done') {
					try {
						const parsed = JSON.parse(data);
						onDone(parsed.session_id ?? '');
					} catch {
						onDone('');
					}
				}
				eventType = '';
			}
		}
	}
}

// ── Notification SSE ──────────────────────────────────────────

/**
 * Subscribe to the live notification SSE stream.
 * Returns a cleanup function to close the connection.
 */
export function subscribeNotifications(
	onNotification: (n: Notification) => void,
	onError?: (e: Event) => void,
): () => void {
	const es = new EventSource(`${API_BASE}/api/notifications`);
	es.addEventListener('notification', (e) => {
		try {
			onNotification(JSON.parse((e as MessageEvent).data));
		} catch {/* ignore malformed */}
	});
	if (onError) es.onerror = onError;
	return () => es.close();
}

// ── Notification History ───────────────────────────────────────

export const getNotificationHistory = (limit = 100, source?: string) => {
	const params = new URLSearchParams({ limit: String(limit) });
	if (source) params.set('source', source);
	return apiFetch<{ notifications: Notification[]; total: number }>(
		`/api/notifications/history?${params}`,
	);
};

export const rateNotification = (id: string, rating: 'useful' | 'partial' | 'useless') =>
	apiFetch<{ status: string }>(`/api/notifications/${id}/rate`, {
		method: 'POST',
		body: JSON.stringify({ rating }),
	});

// ── Goals ─────────────────────────────────────────────────────

export const getGoals = (status?: 'active' | 'completed' | 'abandoned' | 'all') => {
	const params = status ? `?status=${status}` : '';
	return apiFetch<{ goals: Goal[]; available: boolean }>(`/api/goals${params}`);
};

// ── Audit ─────────────────────────────────────────────────────

export const getAudit = (limit = 100) =>
	apiFetch<{ entries: AuditEntry[]; count: number }>(`/api/audit?limit=${limit}`);

export const getMetrics = (from?: string, to?: string) => {
	const params = new URLSearchParams();
	if (from) params.set('from', from);
	if (to) params.set('to', to);
	const qs = params.size ? `?${params}` : '';
	return apiFetch<AuditMetrics>(`/api/metrics${qs}`);
};

// ── Approvals ─────────────────────────────────────────────────

export const getApprovals = () =>
	apiFetch<{ approvals: ApprovalItem[] }>('/api/approvals');

export const approveItem = (id: string) =>
	apiFetch<{ status: string }>(`/api/approvals/${id}/approve`, { method: 'POST' });

export const denyItem = (id: string) =>
	apiFetch<{ status: string }>(`/api/approvals/${id}/deny`, { method: 'POST' });

// ── Memories ──────────────────────────────────────────────────

export const getMemories = (q?: string, limit = 100) => {
	const params = new URLSearchParams({ limit: String(limit) });
	if (q) params.set('q', q);
	return apiFetch<{ memories: MemoryEntry[]; total: number }>(`/api/memories?${params}`);
};

export const getMemory = (id: string) =>
	apiFetch<{ memory: MemoryEntry }>(`/api/memories/${id}`);

export const deleteMemory = (id: string) =>
	apiFetch<void>(`/api/memories/${id}`, { method: 'DELETE' });

// ── Settings ──────────────────────────────────────────────────

export const getSettings = () =>
	apiFetch<{ settings: SettingsSnapshot; config_path: string }>('/api/settings');

export const toggleSetting = (section: string, key: string, value: boolean) =>
	apiFetch<{ status: string; settings: SettingsSnapshot }>('/api/settings/toggle', {
		method: 'PUT',
		body: JSON.stringify({ section, key, value }),
	});

export const updateListSetting = (section: string, key: string, values: string[]) =>
	apiFetch<{ status: string; settings: SettingsSnapshot }>('/api/settings/list', {
		method: 'PUT',
		body: JSON.stringify({ section, key, values }),
	});

export const configureIntegration = (kind: string, fields: [string, string][]) =>
	apiFetch<{ status: string; settings: SettingsSnapshot }>('/api/settings/integration', {
		method: 'POST',
		body: JSON.stringify({ kind, fields }),
	});
