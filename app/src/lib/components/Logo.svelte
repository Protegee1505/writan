<script lang="ts">
  /**
   * The Writan mark: rounded square + W. Colours follow the active theme and
   * accent, so it works on any background. Branding sources live in
   * `branding/` at the repo root.
   */
  let { size = 24 }: { size?: number } = $props();
</script>

<svg viewBox="0 0 96 96" width={size} height={size} aria-label="Writan" role="img">
  <defs>
    <linearGradient id="writan-logo-bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="var(--accent)" />
      <stop offset="1" stop-color="color-mix(in srgb, var(--accent) 68%, #101014)" />
    </linearGradient>
  </defs>
  <rect width="96" height="96" rx="24" fill="url(#writan-logo-bg)" />
  <path
    d="M4.6 6.4 L 8.3 17.6 L 12 9.8 L 15.7 17.6 L 19.4 6.4"
    transform="translate(12 12) scale(3)"
    fill="none"
    stroke="#ffffff"
    stroke-width="2.45"
    stroke-linecap="round"
    stroke-linejoin="round"
  />
</svg>
