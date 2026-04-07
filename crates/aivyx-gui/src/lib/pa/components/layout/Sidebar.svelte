<script lang="ts">
	import { page } from '$app/state';
	import { agentName, pendingApprovals, activeMissions, dashboard } from '$lib/shared/stores/dashboard';
	import { notificationConnected } from '$lib/shared/stores/notifications';

	type NavItem = {
		label: string;
		href: string;
		icon: string;
		badge?: () => number;
	};

	type NavGroup = {
		label?: string;
		items: NavItem[];
	};

	const navGroups: NavGroup[] = [
		{
			items: [
				{ label: 'Home',      href: '/',          icon: 'terminal' },
				{ label: 'Chat',      href: '/chat',      icon: 'forum' },
				{ label: 'Activity',  href: '/activity',  icon: 'electric_bolt' },
				{ label: 'Goals',     href: '/goals',     icon: 'flag' },
				{
					label: 'Approvals',
					href: '/approvals',
					icon: 'approval',
					badge: () => $pendingApprovals,
				},
			],
		},
		{
			label: 'Intelligence',
			items: [
				{
					label: 'Missions',
					href: '/missions',
					icon: 'assignment',
					badge: () => $activeMissions,
				},
				{ label: 'Memory',  href: '/memory', icon: 'memory' },
				{ label: 'Audit',   href: '/audit',  icon: 'shield' },
			],
		},
		{
			label: 'System',
			items: [{ label: 'Settings', href: '/settings', icon: 'settings' }],
		},
	];

	function isActive(href: string): boolean {
		if (href === '/') return page.url.pathname === '/';
		return page.url.pathname.startsWith(href);
	}
</script>

<aside
	class="fixed left-0 top-0 h-screen w-[220px] flex flex-col bg-background border-r border-outline-variant/15 z-50 overflow-y-auto"
>
	<!-- Brand -->
	<div class="p-6 border-b border-outline-variant/10">
		<div class="flex items-center justify-between">
			<h1 class="text-xl font-bold tracking-tighter text-primary font-headline">AIVYX_OS</h1>
			<!-- Live connection dot -->
			<span
				class="w-2 h-2 rounded-full {$notificationConnected
					? 'bg-[var(--color-sage)]'
					: 'bg-on-surface-variant/30'}"
				title={$notificationConnected ? 'Live' : 'Connecting...'}
			></span>
		</div>
		<p class="text-[10px] uppercase tracking-[0.2em] text-on-surface-variant/40 font-mono mt-1">
			v0.9.2 // PA
		</p>
	</div>

	<!-- Navigation -->
	<nav class="flex-1 px-3 py-4 space-y-1">
		{#each navGroups as group, groupIdx}
			{#if group.label}
				<p class="px-3 pt-3 pb-1 text-[9px] uppercase tracking-[0.2em] text-on-surface-variant/30 font-mono">
					{group.label}
				</p>
			{:else if groupIdx > 0}
				<div class="h-px bg-on-surface/5 my-2 mx-3"></div>
			{/if}

			{#each group.items as item}
				{@const badge = item.badge?.() ?? 0}
				<a
					href={item.href}
					class="flex items-center gap-3 px-3 py-2 transition-all rounded-sm {isActive(item.href)
						? 'text-primary font-bold border-r-2 border-primary bg-on-surface/5'
						: 'text-on-surface-variant/60 hover:text-on-surface hover:bg-on-surface/5'}"
				>
					<span class="material-symbols-outlined text-[20px]">{item.icon}</span>
					<span class="text-sm flex-1">{item.label}</span>
					{#if badge > 0}
						<span
							class="text-[10px] font-mono font-bold px-1.5 py-0.5 rounded bg-primary/20 text-primary min-w-[18px] text-center"
						>
							{badge}
						</span>
					{/if}
				</a>
			{/each}
		{/each}
	</nav>

	<!-- Agent Card -->
	<div class="p-4 border-t border-outline-variant/15">
		<div class="flex items-center gap-3 p-2 rounded-lg hover:bg-on-surface/5 transition-colors">
			<div class="w-8 h-8 rounded-full bg-secondary-container flex items-center justify-center text-secondary shrink-0">
				<span class="material-symbols-outlined text-sm">psychology</span>
			</div>
			<div class="overflow-hidden">
				<p class="text-xs font-bold truncate text-primary">{$agentName}</p>
				<p class="text-[10px] text-on-surface-variant/40 font-mono truncate">
					{$dashboard?.autonomy_tier ?? 'Trust'} · {$dashboard?.provider_label ?? '—'}
				</p>
			</div>
		</div>
		{#if $dashboard?.heartbeat_enabled}
			<div class="mt-2 flex items-center gap-2 px-2">
				<span class="w-1.5 h-1.5 rounded-full bg-[var(--color-sage)] animate-pulse"></span>
				<span class="text-[10px] font-mono text-on-surface-variant/40">
					Heartbeat · {$dashboard?.heartbeat_interval_minutes}m
				</span>
			</div>
		{/if}
	</div>
</aside>
