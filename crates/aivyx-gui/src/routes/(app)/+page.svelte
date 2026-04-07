<script lang="ts">
	import { onMount } from 'svelte';
	import { dashboard, dashboardError, dashboardLoading, refreshDashboard } from '$lib/shared/stores/dashboard';
	import { notifications, sourceIcon, sourceColor } from '$lib/shared/stores/notifications';
	import { getNotificationHistory } from '$lib/shared/api/client';
	import type { Notification } from '$lib/shared/api/types';

	let activityFeed: Notification[] = $state([]);

	onMount(async () => {
		try {
			const { notifications: hist } = await getNotificationHistory(12);
			activityFeed = hist;
		} catch {/* show empty */}
	});

	// Live feed: prepend new notifications from SSE
	$effect(() => {
		const latest = $notifications[0];
		if (latest && !activityFeed.find((n) => n.id === latest.id)) {
			activityFeed = [latest, ...activityFeed].slice(0, 12);
		}
	});

	function timeAgo(iso: string): string {
		const secs = Math.max(0, Math.floor((Date.now() - Date.parse(iso)) / 1000));
		if (secs < 60) return `${secs}s ago`;
		if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
		if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
		return `${Math.floor(secs / 86400)}d ago`;
	}

	function healthDot(status: string): string {
		if (status === 'healthy') return 'bg-[var(--color-sage)]';
		if (status === 'n/a') return 'bg-on-surface-variant/20';
		return 'bg-[var(--color-error)]';
	}
</script>

<svelte:head>
	<title>Home — Aivyx PA</title>
</svelte:head>

<!-- Header -->
<section class="mb-8">
	<div class="flex flex-col md:flex-row gap-4 items-end">
		<div class="flex-1">
			<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Dashboard</h1>
			{#if $dashboardError}
				<p class="text-xs text-[var(--color-error)] mt-1 font-mono">
					⚠ Cannot reach PA backend · {$dashboardError}
				</p>
			{:else}
				<p class="text-on-surface-variant/40 text-sm mt-1">
					{$dashboardLoading
						? 'Connecting to agent...'
						: `${$dashboard?.agent_name ?? 'Agent'} is ready · ${$dashboard?.provider_label} — ${$dashboard?.model_name}`}
				</p>
			{/if}
		</div>
		<div class="flex gap-3 bg-surface-container-low p-2 rounded-xl border border-outline-variant/15 ambient-glow">
			<a
				href="/chat"
				class="flex items-center gap-2 px-4 py-2 bg-gradient-to-r from-primary to-primary-container text-on-primary font-bold text-sm rounded-lg hover:scale-[0.98] transition-transform relative overflow-hidden"
			>
				<div class="absolute inset-0 bg-gradient-to-tr from-white/10 to-transparent pointer-events-none"></div>
				<span class="material-symbols-outlined text-sm">add_comment</span>
				New Chat
			</a>
			<a
				href="/missions"
				class="flex items-center gap-2 px-4 py-2 bg-surface-container-high text-on-surface border border-secondary/20 font-bold text-sm rounded-lg hover:bg-surface-bright transition-colors"
			>
				<span class="material-symbols-outlined text-sm">rocket_launch</span>
				Missions
			</a>
		</div>
	</div>
</section>

<div class="grid grid-cols-12 gap-6 pb-8">
	<!-- Stat Cards row -->
	<div class="col-span-12 lg:col-span-8 grid grid-cols-2 md:grid-cols-4 gap-4">
		<div class="animate-fade-in stagger-1 bg-surface-container-high p-5 rounded-xl border border-outline-variant/15">
			<p class="text-[10px] uppercase tracking-widest text-on-surface-variant/40 font-mono mb-1">Goals</p>
			<p class="text-3xl font-headline font-bold text-primary">
				{$dashboardLoading ? '—' : ($dashboard?.goals_active ?? 0)}
			</p>
			<p class="text-xs text-on-surface-variant/40 mt-1">
				of {$dashboard?.goals_total ?? 0} total
			</p>
		</div>

		<div class="animate-fade-in stagger-2 bg-surface-container-high p-5 rounded-xl border border-outline-variant/15">
			<p class="text-[10px] uppercase tracking-widest text-on-surface-variant/40 font-mono mb-1">Memory</p>
			<p class="text-3xl font-headline font-bold text-secondary">
				{$dashboardLoading ? '—' : ($dashboard?.memory_total ?? 0)}
			</p>
			<p class="text-xs text-on-surface-variant/40 mt-1">memories indexed</p>
		</div>

		<div class="animate-fade-in stagger-3 bg-surface-container-high p-5 rounded-xl border border-outline-variant/15">
			<p class="text-[10px] uppercase tracking-widest text-on-surface-variant/40 font-mono mb-1">Missions</p>
			<p class="text-3xl font-headline font-bold text-on-background">
				{$dashboardLoading ? '—' : ($dashboard?.missions_active ?? 0)}
			</p>
			<p class="text-xs text-primary mt-1">
				{($dashboard?.missions_active ?? 0) > 0 ? 'in progress' : 'idle'}
			</p>
		</div>

		<div class="animate-fade-in stagger-4 bg-surface-container-high p-5 rounded-xl border border-outline-variant/15">
			<p class="text-[10px] uppercase tracking-widest text-on-surface-variant/40 font-mono mb-1">Approvals</p>
			<p class="text-3xl font-headline font-bold {($dashboard?.approvals_pending ?? 0) > 0 ? 'text-[var(--color-accent-glow)]' : 'text-[var(--color-sage)]'}">
				{$dashboardLoading ? '—' : ($dashboard?.approvals_pending ?? 0)}
			</p>
			<p class="text-xs text-on-surface-variant/40 mt-1">pending decisions</p>
		</div>
	</div>

	<!-- System Health Panel -->
	<div class="col-span-12 lg:col-span-4 row-span-2 space-y-4">
		<div class="bg-surface-container-high p-6 rounded-xl border border-outline-variant/15 relative overflow-hidden h-full">
			<h4 class="font-headline font-bold text-sm tracking-wide border-b border-outline-variant/15 pb-3 mb-4 font-mono uppercase">
				System Health
			</h4>
			{#if $dashboard}
				<ul class="space-y-4 font-mono text-[11px]">
					<li class="flex justify-between items-center">
						<span class="text-on-surface-variant/40 uppercase">Provider</span>
						<span class="text-primary">{$dashboard.provider_label} · {$dashboard.model_name}</span>
					</li>
					<li class="flex justify-between items-center">
						<span class="text-on-surface-variant/40 uppercase">Autonomy</span>
						<span class="text-on-background bg-surface-container px-2 py-0.5 rounded">{$dashboard.autonomy_tier}</span>
					</li>
					<li class="flex justify-between items-center">
						<span class="text-on-surface-variant/40 uppercase">Memory</span>
						<span class="text-secondary">{$dashboard.memory_total} entries</span>
					</li>
					<li class="flex justify-between items-center">
						<span class="text-on-surface-variant/40 uppercase">Heartbeat</span>
						{#if $dashboard.heartbeat_enabled}
							<span class="text-[var(--color-sage)]">Active · {$dashboard.heartbeat_interval_minutes}m</span>
						{:else}
							<span class="text-on-surface-variant/40">Disabled</span>
						{/if}
					</li>
					<!-- Health subsystems -->
					{#if $dashboard.health}
						{#each Object.entries($dashboard.health).filter(([k]) => k !== 'checked_at') as [key, status]}
							<li class="flex justify-between items-center">
								<span class="text-on-surface-variant/40 uppercase">{key}</span>
								<span class="flex items-center gap-1.5">
									<span class="w-1.5 h-1.5 rounded-full {healthDot((status as any).status)}"></span>
									<span class={(status as any).status === 'healthy' ? 'text-[var(--color-sage)]' : 'text-on-surface-variant/40'}>
										{(status as any).status}
									</span>
								</span>
							</li>
						{/each}
					{/if}
				</ul>
			{:else}
				<p class="text-xs text-on-surface-variant/30 font-mono">Connecting...</p>
			{/if}
		</div>
	</div>

	<!-- Activity Feed -->
	<div class="col-span-12 lg:col-span-8 bg-surface-container-low/50 rounded-2xl border border-outline-variant/15 p-6 backdrop-blur-sm animate-fade-in stagger-5">
		<div class="flex justify-between items-center mb-6">
			<h3 class="font-headline font-bold text-lg">Recent Activity</h3>
			<a href="/activity" class="text-xs font-mono text-on-surface-variant/40 hover:text-primary transition-colors flex items-center gap-1">
				VIEW ALL <span class="material-symbols-outlined text-xs">chevron_right</span>
			</a>
		</div>

		{#if activityFeed.length === 0}
			<p class="text-sm text-on-surface-variant/30 font-mono">No activity yet. The agent will log events here.</p>
		{:else}
			<div class="space-y-3">
				{#each activityFeed as notif (notif.id)}
					<div class="group relative bg-surface-container-high/40 p-4 rounded-xl hover:bg-surface-container-high transition-all border border-transparent hover:border-primary/20 animate-fade-in">
						<div class="flex items-start justify-between gap-4">
							<div class="flex gap-3 items-start min-w-0">
								<!-- Source icon with TUI-matching color -->
								<div
									class="w-9 h-9 rounded-lg flex items-center justify-center border border-outline-variant/15 shrink-0"
									style="color: {sourceColor(notif.source)}; background: color-mix(in srgb, {sourceColor(notif.source)} 12%, transparent)"
								>
									<span class="material-symbols-outlined text-sm">{sourceIcon(notif.source)}</span>
								</div>
								<div class="min-w-0">
									<h5 class="text-sm font-bold truncate">{notif.title}</h5>
									<p class="text-xs text-on-surface-variant/40 mt-0.5 line-clamp-1">{notif.body}</p>
									{#if notif.requires_approval}
										<a href="/approvals" class="inline-flex items-center gap-1 text-[10px] font-mono text-[var(--color-accent-glow)] hover:underline mt-1">
											<span class="material-symbols-outlined text-[10px]">approval</span>
											Requires approval
										</a>
									{/if}
								</div>
							</div>
							<div class="shrink-0 text-right">
								<span class="text-[10px] font-mono text-on-surface-variant/30 whitespace-nowrap">{timeAgo(notif.timestamp)}</span>
								<p class="text-[9px] font-mono mt-0.5" style="color: {sourceColor(notif.source)}">
									{notif.source}
								</p>
							</div>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>
