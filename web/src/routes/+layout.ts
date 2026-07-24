// Pure SPA — the Rust server serves index.html for every non-asset path and
// never looks for per-route HTML, so prerendering would only emit dead files.
export const ssr = false;
export const prerender = false;
