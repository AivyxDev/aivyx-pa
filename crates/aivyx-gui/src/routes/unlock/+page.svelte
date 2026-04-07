<script lang="ts">
	import AmbientBackground from '$lib/shared/components/AmbientBackground.svelte';
	import CandleLogo from '$lib/shared/components/CandleLogo.svelte';
	import Button from '$lib/shared/components/Button.svelte';
	import { goto } from '$app/navigation';

	let passphrase = $state('');
	let showPassword = $state(false);
	let error = $state(false);

	function handleUnlock() {
		if (passphrase.length >= 8) {
			goto('/');
		} else {
			error = true;
			setTimeout(() => (error = false), 600);
		}
	}
</script>

<svelte:head>
	<title>Unlock — Aivyx</title>
</svelte:head>

<AmbientBackground />

<main class="relative z-10 flex items-center justify-center min-h-screen px-6">
	<div class="w-full max-w-md relative">
		<!-- Asymmetric main container -->
		<div
			class="bg-surface-container-low backdrop-blur-xl rounded-xl p-8 ambient-glow border-l border-t border-outline-variant/20 relative overflow-hidden"
			class:animate-shake={error}
		>
			<!-- Internal gradient -->
			<div class="absolute inset-0 bg-gradient-to-br from-surface-container-highest/10 to-transparent pointer-events-none"></div>

			<!-- Candle Logo Section -->
			<div class="flex flex-col items-center mb-10 relative">
				<div class="w-20 h-20 bg-surface-container-highest rounded-full flex items-center justify-center mb-6 relative group">
					<div class="absolute inset-0 rounded-full bg-primary/20 blur-xl animate-pulse-glow"></div>
					<CandleLogo size={48} animate={true} />
				</div>
				<h1 class="font-headline text-3xl font-bold tracking-tighter text-on-surface">AIVYX</h1>
				<p class="font-mono text-[10px] uppercase tracking-[0.2em] text-secondary text-center opacity-80 mt-2">
					Your AI. Your Machine. Your Rules.
				</p>
			</div>

			<!-- Form -->
			<form class="space-y-6 relative" onsubmit={(e) => { e.preventDefault(); handleUnlock(); }}>
				<div class="space-y-2">
					<div class="flex justify-between items-end mb-1">
						<label class="font-mono text-[10px] uppercase tracking-widest text-on-surface-variant">
							Access Passphrase
						</label>
						<span class="font-mono text-[10px] text-primary/60">Session: 12m ago</span>
					</div>
					<div class="relative group">
						<input
							class="w-full bg-surface-container-lowest border-none ring-1 ring-outline-variant/30 focus:ring-primary/50 rounded-lg py-4 px-5 font-mono text-on-surface placeholder:text-on-surface-variant/30 transition-all outline-none"
							type={showPassword ? 'text' : 'password'}
							placeholder="••••••••••••"
							bind:value={passphrase}
						/>
						<button
							type="button"
							class="absolute right-4 top-1/2 -translate-y-1/2 text-on-surface-variant/40 hover:text-on-surface transition-colors"
							onclick={() => (showPassword = !showPassword)}
						>
							<span class="material-symbols-outlined text-lg">
								{showPassword ? 'visibility_off' : 'visibility'}
							</span>
						</button>
					</div>
					<!-- Strength meter -->
					<div class="flex gap-1 pt-2">
						<div class="h-0.5 flex-1 rounded-full transition-colors" class:bg-primary={passphrase.length >= 2} class:bg-surface-container-highest={passphrase.length < 2}></div>
						<div class="h-0.5 flex-1 rounded-full transition-colors" class:bg-primary={passphrase.length >= 5} class:bg-surface-container-highest={passphrase.length < 5}></div>
						<div class="h-0.5 flex-1 rounded-full transition-colors" class:bg-primary={passphrase.length >= 8} class:bg-surface-container-highest={passphrase.length < 8}></div>
						<div class="h-0.5 flex-1 rounded-full transition-colors" class:bg-primary={passphrase.length >= 12} class:bg-surface-container-highest={passphrase.length < 12}></div>
					</div>
				</div>

				<Button variant="primary" class="w-full py-4" onclick={handleUnlock}>
					Unlock Workspace
					<span class="material-symbols-outlined text-sm">arrow_forward</span>
				</Button>
			</form>

			<!-- Footer actions -->
			<div class="mt-8 pt-6 border-t border-outline-variant/10 flex flex-col gap-4 items-center">
				<button class="text-[11px] font-mono uppercase tracking-widest text-on-secondary-container hover:text-secondary transition-colors flex items-center gap-2">
					<span class="material-symbols-outlined text-xs">refresh</span>
					Reset Vault Identity
				</button>
			</div>

			<!-- Decorative metadata -->
			<div class="absolute bottom-4 right-4 pointer-events-none opacity-20">
				<span class="font-mono text-[8px] tracking-tighter">SEC_VAULT_V0.9.2</span>
			</div>
		</div>

		<!-- Asymmetric decorative elements -->
		<div class="absolute -top-12 -right-12 w-24 h-24 border border-secondary/10 rounded-full flex items-center justify-center rotate-12 pointer-events-none">
			<div class="w-16 h-16 border border-secondary/20 rounded-full flex items-center justify-center">
				<div class="w-8 h-8 border border-secondary/30 rounded-full"></div>
			</div>
		</div>
		<div class="absolute -bottom-8 -left-8 font-mono text-[10px] text-on-surface-variant/20 tracking-widest pointer-events-none" style="writing-mode: vertical-rl; transform: rotate(180deg);">
			PROTOCOL_AIVYX_001
		</div>
	</div>
</main>
