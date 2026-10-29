//! Builds the SvelteKit UI into `static/` so `cargo build` is the whole build.
//!
//! `src/web_assets.rs` declares `#[derive(Embed)] #[folder = "static/"]`, so
//! that directory has to exist and be current *before* rustc runs. That used to
//! be `./build-web.sh`, run by hand and by CI, which meant a fresh clone did not
//! compile and a stale `static/` shipped silently.
//!
//! This is a port of that script, not a replacement for its checks — the
//! lockfile fallback and the bundle smoke test below are the reason it was more
//! than two `bun` calls. `build-web.sh` is kept for frontend-only iteration,
//! where running cargo just to rebuild the UI is the long way round.
//!
//! Set `TAPEDECK_SKIP_WEB_BUILD=1` to skip all of this. Rust-only work
//! (`cargo check` in an editor, `cargo clippy`) does not need bun on PATH, and
//! without an escape hatch a missing bun would make the crate uncompilable
//! rather than merely un-runnable.

use std::path::Path;
use std::process::Command;

fn main() {
    // Emitting any rerun-if-changed narrows cargo's default of "rerun when any
    // file in the package changed" to just these. Everything the UI build reads
    // has to be listed, or an edit to it will not trigger a rebuild.
    for path in [
        "web/src",
        "web/static",
        "web/package.json",
        "web/bun.lock",
        "web/svelte.config.js",
        "web/vite.config.ts",
        "web/tailwind.config.js",
        "web/postcss.config.js",
        "web/tsconfig.json",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=TAPEDECK_SKIP_WEB_BUILD");
    // The bundle's version stamp (svelte.config.js). CI sets it to the commit
    // so the amd64 and arm64 builds of one commit embed the same bytes; without
    // a rerun here, a commit touching only Rust would keep the previous
    // commit's bundle in a persistent target dir, stamped with the old sha.
    println!("cargo:rerun-if-env-changed=TAPEDECK_UI_VERSION_NAME");

    if std::env::var("TAPEDECK_SKIP_WEB_BUILD").is_ok_and(|v| v != "0") {
        println!("cargo:warning=TAPEDECK_SKIP_WEB_BUILD set — using the existing static/");
        return;
    }

    if Command::new("bun").arg("--version").output().is_err() {
        panic!(
            "bun is required to build the embedded UI but was not found on PATH.\n\
             Install it with:  curl -fsSL https://bun.sh/install | bash\n\
             To build the Rust side alone against an existing static/, set \
             TAPEDECK_SKIP_WEB_BUILD=1."
        );
    }

    // bun.lock is committed on purpose. A frozen install keeps the toolchain
    // pinned; drifting off it is how vite and vite-plugin-svelte once landed on
    // incompatible majors, which compiled every onMount out of the bundle
    // without failing the build. Fall back rather than hard-stop, because a
    // legitimately edited package.json should not block a build — but say so,
    // because the updated lockfile needs committing.
    let frozen = run("bun", &["install", "--frozen-lockfile"]);
    if !frozen {
        println!(
            "cargo:warning=web/bun.lock is out of date with package.json — re-resolving. \
             Commit the updated web/bun.lock."
        );
        if !run("bun", &["install"]) {
            panic!("bun install failed");
        }
    }

    if !run("bun", &["run", "build"]) {
        panic!("Frontend build failed");
    }

    verify_bundle();
}

fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .current_dir("web")
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("failed to run `{program} {}`: {e}", args.join(" ")))
        .success()
}

/// The root layout's `onMount` boots the entire SPA — it is the auth gate that
/// redirects to `/setup` or `/login`. If a broken toolchain drops lifecycle
/// callbacks the build still succeeds and ships a page that spins forever, with
/// nothing in the console. Assert the call survived into the bundle instead of
/// finding out in a browser.
fn verify_bundle() {
    let app = Path::new("static/_app");
    let found = walk(app)
        .unwrap_or_default()
        .iter()
        .any(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains("auth/status")));
    if !found {
        panic!(
            "Built bundle never references /api/v1/auth/status.\n\
             The root layout's onMount was compiled away — the SPA would boot to a \
             blank page.\n\
             Check that svelte, vite and @sveltejs/vite-plugin-svelte are on mutually \
             compatible majors (`cd web && bun pm ls`)."
        );
    }
}

fn walk(dir: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            out.extend(walk(&path)?);
        } else {
            out.push(path);
        }
    }
    Ok(out)
}
