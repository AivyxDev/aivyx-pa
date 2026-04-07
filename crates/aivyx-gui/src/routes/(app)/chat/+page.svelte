<script lang="ts">
	let messages = $state([
		{ role: 'assistant', content: 'Good morning! I\'ve processed your overnight emails. 3 urgent items need your attention. Would you like me to summarize them?', time: '09:12' },
		{ role: 'user', content: 'Yes, go ahead.', time: '09:13' },
		{ role: 'assistant', content: 'Here\'s your urgent email summary:\n\n1. **Meeting reschedule** — Your 2pm with the design team moved to 3pm\n2. **PR review needed** — Backend team needs approval on the auth refactor\n3. **Server alert** — Monitoring flagged high memory on staging (resolved itself)\n\nWant me to respond to any of these?', time: '09:13' }
	]);
	let input = $state('');

	function sendMessage() {
		if (!input.trim()) return;
		messages = [...messages, { role: 'user', content: input, time: new Date().toLocaleTimeString('en-US', { hour: '2-digit', minute: '2-digit', hour12: false }) }];
		input = '';
	}
</script>

<svelte:head>
	<title>Chat — Aivyx PA</title>
</svelte:head>

<div class="flex flex-col h-full -m-8">
	<!-- Messages area -->
	<div class="flex-1 overflow-y-auto p-8 space-y-4">
		{#each messages as message}
			<div class="flex gap-4 max-w-3xl {message.role === 'user' ? 'ml-auto flex-row-reverse' : ''}">
				<!-- Avatar -->
				<div class="w-8 h-8 rounded-full flex-shrink-0 flex items-center justify-center {message.role === 'assistant' ? 'bg-primary/20 text-primary' : 'bg-secondary-container text-secondary'}">
					<span class="material-symbols-outlined text-sm">
						{message.role === 'assistant' ? 'smart_toy' : 'person'}
					</span>
				</div>
				<!-- Bubble -->
				<div class="flex-1 {message.role === 'user' ? 'text-right' : ''}">
					<div class="inline-block text-left p-4 rounded-xl text-sm leading-relaxed {message.role === 'assistant' ? 'bg-surface-container-high border border-outline-variant/15' : 'bg-primary/10 border border-primary/20'}">
						{@html message.content.replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>').replace(/\n/g, '<br/>')}
					</div>
					<p class="text-[10px] font-mono text-on-surface-variant/30 mt-1">{message.time}</p>
				</div>
			</div>
		{/each}
	</div>

	<!-- Input area -->
	<div class="border-t border-outline-variant/15 p-6 bg-surface-container-low/50 backdrop-blur-sm">
		<form class="flex items-end gap-4 max-w-3xl mx-auto" onsubmit={(e) => { e.preventDefault(); sendMessage(); }}>
			<div class="flex-1 relative">
				<textarea
					class="w-full bg-surface-container-lowest ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-xl py-3 px-5 font-body text-sm text-on-surface placeholder:text-on-surface-variant/30 outline-none resize-none"
					placeholder="Type a message..."
					rows="1"
					bind:value={input}
					onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendMessage(); }}}
				></textarea>
			</div>
			<button type="submit" class="w-10 h-10 rounded-xl bg-gradient-to-r from-primary to-primary-container text-on-primary flex items-center justify-center hover:scale-95 transition-transform">
				<span class="material-symbols-outlined text-lg">arrow_upward</span>
			</button>
		</form>
	</div>
</div>
