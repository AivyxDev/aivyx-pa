// ── Aivyx PA API Types ──────────────────────────────────────────
// TypeScript interfaces matching the PA HTTP API (api.rs) exactly.
// Port: 3100 (default) · Base: http://127.0.0.1:3100

// ── Health ─────────────────────────────────────────────────────

export interface SubsystemHealth {
	status: 'healthy' | 'n/a' | 'degraded';
	detail?: string;
}

export interface HealthResponse {
	provider: SubsystemHealth;
	email: SubsystemHealth;
	config: SubsystemHealth;
	disk: SubsystemHealth;
	checked_at: string;
}

// ── Dashboard ──────────────────────────────────────────────────

export interface DashboardResponse {
	agent_name: string;
	provider_label: string;
	model_name: string;
	autonomy_tier: string;
	goals_active: number;
	goals_total: number;
	missions_active: number;
	missions_total: number;
	approvals_pending: number;
	memory_total: number;
	heartbeat_enabled: boolean;
	heartbeat_interval_minutes: number;
	health: HealthResponse;
}

// ── Chat / Sessions ────────────────────────────────────────────

export interface ChatSession {
	id: string;
	title: string;
	turn_count: number;
	created_at: string;
	updated_at: string;
}

export interface ChatMessage {
	role: 'you' | 'assistant';
	content: string;
}

// SSE events from POST /api/chat
export type ChatEvent =
	| { type: 'token'; data: string }
	| { type: 'done'; data: { session_id: string } };

// ── Goals ─────────────────────────────────────────────────────

export interface Goal {
	id: string;
	description: string;
	success_criteria: string;
	status: 'Active' | 'Completed' | 'Abandoned';
	priority: 'Critical' | 'High' | 'Medium' | 'Low';
	progress: number;           // 0.0–1.0
	tags: string[];
	parent_id?: string;
	deadline?: string;          // ISO 8601
	cooldown_until?: string;    // ISO 8601
	failure_count: number;
	created_at: string;
	updated_at: string;
}

// ── Missions ───────────────────────────────────────────────────

export interface MissionStep {
	id: string;
	name: string;
	status: 'Pending' | 'Running' | 'Completed' | 'Failed' | 'Skipped';
	result?: string;
	error?: string;
}

export interface Mission {
	id: string;
	title: string;
	objective: string;
	status: 'Pending' | 'Running' | 'Paused' | 'Completed' | 'Failed' | 'Cancelled';
	steps: MissionStep[];
	current_step_index?: number;
	created_at: string;
	completed_at?: string;
	error?: string;
}

// ── Approvals ──────────────────────────────────────────────────

export type ApprovalStatus = 'pending' | 'approved' | 'denied';

export interface Notification {
	id: string;
	title: string;
	body: string;
	source: string;
	timestamp: string;          // ISO 8601
	requires_approval: boolean;
}

export interface ApprovalItem {
	notification: Notification;
	status: ApprovalStatus;
	resolved_at?: string;
}

// ── Audit ─────────────────────────────────────────────────────

export interface AuditEntry {
	sequence_number: number;
	timestamp: string;
	event: Record<string, unknown>;  // variant object from AuditEvent enum
	hmac: string;
}

export interface AuditMetrics {
	llm_calls: number;
	tool_executions: number;
	denials: number;
	agent_turns: number;
	tokens_used: number;
}

// ── Memory ────────────────────────────────────────────────────

export interface MemoryEntry {
	id: string;
	content: string;
	tags: string[];
	kind: string;
	created_at: string;
	updated_at: string;
}

// ── Settings ──────────────────────────────────────────────────

export interface ScheduleEntry {
	name: string;
	cron: string;
	enabled: boolean;
}

export interface PersonaDimensions {
	warmth: number;
	formality: number;
	verbosity: number;
	humor: number;
	confidence: number;
	curiosity: number;
}

export interface SettingsSnapshot {
	// Provider
	provider_label: string;
	model_name: string;
	provider_base_url?: string;
	embedding_model?: string;
	embedding_dimensions?: number;
	// Autonomy
	autonomy_tier: string;
	max_tool_calls_per_min: number;
	max_cost_usd: number;
	require_approval_destructive: boolean;
	// Heartbeat
	heartbeat_enabled: boolean;
	heartbeat_interval: number;
	heartbeat_can_reflect: boolean;
	heartbeat_can_consolidate: boolean;
	heartbeat_can_analyze_failures: boolean;
	heartbeat_can_extract_knowledge: boolean;
	heartbeat_can_plan_review: boolean;
	heartbeat_can_strategy_review: boolean;
	heartbeat_can_track_mood: boolean;
	heartbeat_can_encourage: boolean;
	heartbeat_can_track_milestones: boolean;
	heartbeat_notification_pacing: boolean;
	// Agent
	agent_name: string;
	agent_persona: string;
	agent_skills: string[];
	has_custom_soul: boolean;
	// Integrations
	email_configured: boolean;
	telegram_configured: boolean;
	matrix_configured: boolean;
	calendar_configured: boolean;
	contacts_configured: boolean;
	vault_configured: boolean;
	signal_configured: boolean;
	sms_configured: boolean;
	// Memory
	max_memories: number;
	session_max_age_hours: number;
	use_graph_recall: boolean;
	// Persona
	persona_dimensions?: PersonaDimensions;
	// Tools
	tool_count: number;
	tool_discovery_mode?: string;
	mcp_servers: string[];
	// Schedules
	schedules: [string, string, boolean][];  // [name, cron, enabled]
}
