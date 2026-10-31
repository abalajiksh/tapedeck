<!--
  Tapedeck cassette. The shell is currentColor and every hole is a real
  cutout, so it follows whatever text colour the theme gives its parent:
  cream on the dark sidebar, espresso on the light one. No theme logic here.

  <TapedeckLogo />                 48 px wide, decorative (beside the wordmark)
  <TapedeckLogo width={96} title="Tapedeck" />   standalone, announced

  Optional overrides, set on any ancestor:
    --td-logo-tape   reels and tape   (default #E9A23B)
    --td-logo-label  label stripe     (default #C8553D)
-->
<script>
  /** Rendered width in px; height follows the 64:40 cassette. */
  export let width = 48;
  /** Accessible name. Leave empty when a visible "Tapedeck" sits beside it. */
  export let title = '';
</script>

<svg
  class="td-logo"
  viewBox="0 0 64 40"
  width={width}
  height={width * 0.625}
  role={title ? 'img' : undefined}
  aria-label={title || undefined}
  aria-hidden={title ? undefined : 'true'}
  focusable="false"
>
  <path class="shell" fill-rule="evenodd" d="M5 0H59A5 5 0 0 1 64 5V35A5 5 0 0 1 59 40H5A5 5 0 0 1 0 35V5A5 5 0 0 1 5 0ZM21 12H43A7 7 0 0 1 50 19V19A7 7 0 0 1 43 26H21A7 7 0 0 1 14 19V19A7 7 0 0 1 21 12ZM17 40L20.5 32H43.5L47 40Z" />
  <path class="label" d="M8.5 4.5H55.5A1.5 1.5 0 0 1 57 6V6A1.5 1.5 0 0 1 55.5 7.5H8.5A1.5 1.5 0 0 1 7 6V6A1.5 1.5 0 0 1 8.5 4.5Z" />
  <path class="tape" fill-rule="evenodd" d="M16 19A6 6 0 1 0 28 19A6 6 0 1 0 16 19ZM19.7 19A2.3 2.3 0 1 0 24.3 19A2.3 2.3 0 1 0 19.7 19ZM38 19A4 4 0 1 0 46 19A4 4 0 1 0 38 19ZM40.2 19A1.8 1.8 0 1 0 43.8 19A1.8 1.8 0 1 0 40.2 19Z" />
  <path class="tape" d="M23.5 35H40.5A1 1 0 0 1 41.5 36V36A1 1 0 0 1 40.5 37H23.5A1 1 0 0 1 22.5 36V36A1 1 0 0 1 23.5 35Z" />
</svg>

<style>
  .td-logo { display: block; flex-shrink: 0; }
  .shell { fill: currentColor; }
  .label { fill: var(--td-logo-label, #C8553D); }
  .tape  { fill: var(--td-logo-tape, #E9A23B); }
</style>
