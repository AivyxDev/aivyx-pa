<script lang="ts">
	let { size = 48, animate = false }: { size?: number; animate?: boolean } = $props();
</script>

<div class="relative inline-flex items-center justify-center" class:animate-flicker={animate}>
	<svg
		class="text-primary relative z-10"
		fill="none"
		height={size}
		width={size}
		viewBox="0 0 24 24"
		xmlns="http://www.w3.org/2000/svg"
	>
		<!-- Candle body -->
		<path
			d="M12 10V22M12 10C12 10 10 8.5 10 7C10 5.89543 10.8954 5 12 5C13.1046 5 14 5.89543 14 7C14 8.5 12 10 12 10Z"
			stroke="currentColor"
			stroke-linecap="round"
			stroke-linejoin="round"
			stroke-width="1.5"
		/>
		<!-- Flame tip -->
		<path
			d="M12 2C12 2 11.5 3.5 11.5 4C11.5 4.27614 11.7239 4.5 12 4.5C12.2761 4.5 12.5 4.27614 12.5 4C12.5 3.5 12 2 12 2Z"
			fill="currentColor"
		/>
		<!-- Base -->
		<path d="M8 22H16" stroke="currentColor" stroke-linecap="round" stroke-width="1.5" />
	</svg>
</div>
