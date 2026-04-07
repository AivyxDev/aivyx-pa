<svelte:head>
	<title>Audit — Aivyx PA</title>
</svelte:head>

<section class="mb-8">
	<div class="flex items-end justify-between">
		<div>
			<h1 class="text-3xl font-headline font-bold text-on-background tracking-tight">Audit Trail</h1>
			<p class="text-on-surface-variant/40 text-sm mt-1">HMAC chain-verified cryptographic audit trail.</p>
		</div>
		<div class="flex items-center gap-3">
			<span class="flex items-center gap-2 font-mono text-[10px] text-sage bg-sage/10 px-3 py-1 rounded-lg">
				<span class="material-symbols-outlined text-xs">verified</span>
				Chain: Verified
			</span>
		</div>
	</div>
</section>

<div class="grid grid-cols-12 gap-6">
	<!-- Timeline -->
	<div class="col-span-12 lg:col-span-8 bg-surface-container-low/50 rounded-2xl border border-outline-variant/15 p-6">
		<div class="space-y-6 relative before:content-[''] before:absolute before:left-[11px] before:top-2 before:bottom-0 before:w-px before:bg-on-surface/5">
			{#each [
				{ time: '14:22:04', seq: '#4821', title: 'Tool execution: email_read', detail: 'Agent read 18 emails from IMAP inbox. Duration: 2.4s', type: 'tool' },
				{ time: '14:21:58', seq: '#4820', title: 'LLM call: qwen3:14b', detail: 'Tokens in: 1,240 | Tokens out: 380 | Cost: $0.00', type: 'llm' },
				{ time: '14:15:00', seq: '#4819', title: 'Heartbeat: Cycle #142', detail: 'Context scan: calendar, email, goals. 2 suggestions generated.', type: 'heartbeat' },
				{ time: '13:45:12', seq: '#4818', title: 'Security: Capability check', detail: 'Agent requested file_write. Checked against Trust tier. Allowed.', type: 'security' },
				{ time: '13:44:02', seq: '#4817', title: 'Tool execution: file_read', detail: 'Read ~/Documents/notes.md (2.1KB). Duration: 0.1s', type: 'tool' },
				{ time: '13:30:00', seq: '#4816', title: 'Heartbeat: Cycle #141', detail: 'Context fusion: correlated calendar with email deadlines.', type: 'heartbeat' },
				{ time: '12:10:55', seq: '#4815', title: 'Auth: Session unlock', detail: 'Passphrase verified. JWT issued. TTL: 24h.', type: 'auth' }
			] as event}
				<div class="relative pl-8">
					<div class="absolute left-0 top-1.5 w-[22px] h-[22px] rounded-full border-4 border-surface-container-low flex items-center justify-center
						{event.type === 'tool' ? 'bg-primary/20' :
						 event.type === 'llm' ? 'bg-secondary/20' :
						 event.type === 'heartbeat' ? 'bg-sage/20' :
						 event.type === 'security' ? 'bg-accent-glow/20' :
						 'bg-teal/20'}">
						<div class="w-1.5 h-1.5 rounded-full
							{event.type === 'tool' ? 'bg-primary' :
							 event.type === 'llm' ? 'bg-secondary' :
							 event.type === 'heartbeat' ? 'bg-sage' :
							 event.type === 'security' ? 'bg-accent-glow' :
							 'bg-teal'}"></div>
					</div>
					<div class="flex items-center gap-3">
						<p class="text-[11px] font-mono text-on-surface-variant/30">{event.time}</p>
						<span class="text-[9px] font-mono text-on-surface-variant/20 bg-surface-container px-1.5 py-0.5 rounded">{event.seq}</span>
					</div>
					<p class="text-xs font-bold mt-0.5">{event.title}</p>
					<p class="text-[10px] text-on-surface-variant/40 mt-1">{event.detail}</p>
				</div>
			{/each}
		</div>
	</div>

	<!-- Stats sidebar -->
	<div class="col-span-12 lg:col-span-4 space-y-4">
		<div class="bg-surface-container-high p-6 rounded-xl border border-outline-variant/15">
			<h4 class="font-headline font-bold text-sm tracking-wide mb-4">CHAIN INTEGRITY</h4>
			<ul class="space-y-3 font-mono text-[11px]">
				<li class="flex justify-between"><span class="text-on-surface-variant/40">TOTAL EVENTS</span><span>4,821</span></li>
				<li class="flex justify-between"><span class="text-on-surface-variant/40">HMAC VALID</span><span class="text-sage">100%</span></li>
				<li class="flex justify-between"><span class="text-on-surface-variant/40">LAST VERIFIED</span><span class="text-primary">Just now</span></li>
				<li class="flex justify-between"><span class="text-on-surface-variant/40">CHAIN ROOT</span><span class="text-secondary truncate ml-4">0x8f4b…d5e2</span></li>
			</ul>
		</div>
		<div class="bg-surface-container-high p-6 rounded-xl border border-outline-variant/15">
			<h4 class="font-headline font-bold text-sm tracking-wide mb-4">EVENT BREAKDOWN</h4>
			<ul class="space-y-3 font-mono text-[11px]">
				<li class="flex justify-between items-center"><span class="flex items-center gap-2"><span class="w-2 h-2 rounded-full bg-primary"></span>Tool Calls</span><span>2,104</span></li>
				<li class="flex justify-between items-center"><span class="flex items-center gap-2"><span class="w-2 h-2 rounded-full bg-secondary"></span>LLM Calls</span><span>1,892</span></li>
				<li class="flex justify-between items-center"><span class="flex items-center gap-2"><span class="w-2 h-2 rounded-full bg-sage"></span>Heartbeats</span><span>612</span></li>
				<li class="flex justify-between items-center"><span class="flex items-center gap-2"><span class="w-2 h-2 rounded-full bg-accent-glow"></span>Security</span><span>148</span></li>
				<li class="flex justify-between items-center"><span class="flex items-center gap-2"><span class="w-2 h-2 rounded-full bg-teal"></span>Auth</span><span>65</span></li>
			</ul>
		</div>
	</div>
</div>
