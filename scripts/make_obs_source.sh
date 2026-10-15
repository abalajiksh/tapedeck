#!/usr/bin/env bash
# ============================================================================
# make_obs_source.sh — assemble the openSUSE Build Service upload for tapedeck
#
# OBS builds run with NO network, and the two things a normal `cargo build`
# fetches are therefore put into one source tarball here:
#
#   tapedeck-<ver>/            git archive of <ref>
#   tapedeck-<ver>/static/     the SvelteKit UI, built here with bun
#   tapedeck-<ver>/vendor/     every crate in Cargo.lock (`cargo vendor`)
#   tapedeck-<ver>/lindera-cache/  the dictionary archives lindera's build
#                              scripts would otherwise download
#   tapedeck-<ver>/.cargo/config.toml   source replacement onto vendor/, and
#                              LINDERA_BUILD_DICTIONARY_CACHE_DIR
#
# The UI is prebuilt because no distro OBS targets packages bun. The package
# build sets TAPEDECK_SKIP_WEB_BUILD=1, which is build.rs's own escape hatch,
# and the bundle smoke test build.rs would have run is run here instead.
#
# Crates only compiled for other platforms (windows-*, wasm, …) are replaced
# with empty stubs, which is most of the vendor tree's size. A stub keeps its
# Cargo.toml and its checksum's package hash, so `--locked` resolution is
# unchanged; only its sources are gone. Which crates count as "used" comes
# from `cargo metadata --filter-platform` for every Linux target in LINUX_TARGETS
# — add one there before enabling a new architecture on OBS.
#
# The tarball is DETERMINISTIC (sorted entries, fixed mtime/owner, gzip
# mtime 0, and the UI's version name pinned to the commit): regenerating from
# the same ref gives the same sha256, which is what lets the release job skip
# a tag OBS already carries. Checked by packaging one ref twice.
#
# Usage:
#   [PREV_DIR=dir] scripts/make_obs_source.sh OUTDIR [REF]
#     OUTDIR    where the upload set is written (created; must be empty or new)
#     REF       git ref to package (default HEAD). The version is read from
#               Cargo.toml AT THAT REF, so package a tag for a release.
#     PREV_DIR  an `osc co` of the package as currently published. Its
#               tapedeck.changes / debian.changelog are kept BELOW the new
#               entry, so the history accumulates instead of being replaced
#               each release. If they already hold an entry for this version
#               (a re-run), they are reused unchanged rather than duplicated.
#
# The changelog line is the tag's annotation subject ("vX.Y.Z — ..."), so
# writing a good tag message IS writing the package changelog.
#
# Output (upload all of it to the OBS package):
#   tapedeck-<ver>.tar.gz  tapedeck.spec  tapedeck.changes
#   tapedeck.dsc  debian.control  debian.rules  debian.changelog
#   debian.copyright  PKGBUILD
# ============================================================================

set -euo pipefail

OUT="${1:?usage: $0 OUTDIR [REF]}"
REF="${2:-HEAD}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TPL="$ROOT/packaging/obs"

# Overridable only so the tarball can be rehearsed on a non-Linux machine
# (add that host's triple); an OBS upload always uses the default.
read -r -a LINUX_TARGETS <<< "${OBS_TARGETS:-x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu}"

for tool in git cargo bun python3; do
    command -v "$tool" >/dev/null || { echo "error: $tool is required" >&2; exit 1; }
done

cd "$ROOT"
git rev-parse --verify --quiet "$REF^{commit}" >/dev/null \
    || { echo "error: unknown ref '$REF'" >&2; exit 1; }
# Uncommitted edits are NOT packaged (git archive reads the ref), which is
# easy to forget when packaging HEAD -- say so rather than surprise.
if [ "$REF" = HEAD ] && ! git diff --quiet HEAD -- . ':!temp'; then
    echo "warning: working tree has uncommitted changes; they are NOT in the tarball" >&2
fi

# The [package] version specifically, not the first `version =` in the file.
VERSION="$(git show "$REF:Cargo.toml" | awk '
    /^\[/ { in_pkg = ($0 == "[package]") }
    in_pkg && /^version *=/ { gsub(/.*= *"|".*/, ""); print; exit }')"
[ -n "$VERSION" ] || { echo "error: could not read the version from $REF:Cargo.toml" >&2; exit 1; }
NAME="tapedeck-$VERSION"
SRC_SHA="$(git rev-parse "$REF^{commit}")"
echo ">> tapedeck $VERSION (ref $REF, ${SRC_SHA:0:7})"

mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"
[ -z "$(ls -A "$OUT")" ] || { echo "error: $OUT is not empty" >&2; exit 1; }
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
STAGE="$WORK/$NAME"

# ── tapedeck itself ─────────────────────────────────────────────────────────
git archive --prefix="$NAME/" "$REF" | tar -x -C "$WORK"
rm -rf "$STAGE/temp"

# ── the UI, built once here ─────────────────────────────────────────────────
echo ">> building the UI"
(
    cd "$STAGE/web"
    bun install --frozen-lockfile >/dev/null
    # Replaces SvelteKit's Date.now() version name, so the bundle -- and with
    # it the tarball -- is the same bytes every time this ref is packaged.
    TAPEDECK_UI_VERSION_NAME="$SRC_SHA" bun run build >/dev/null
)
# Same assertion as build.rs::verify_bundle: the root layout's onMount is the
# auth gate, and a toolchain that drops it still builds a blank-page SPA.
grep -rqF 'auth/status' "$STAGE/static/_app" \
    || { echo "error: built bundle never references auth/status — see build.rs::verify_bundle" >&2; exit 1; }
rm -rf "$STAGE/web/node_modules" "$STAGE/web/.svelte-kit"

# ── the crates ──────────────────────────────────────────────────────────────
echo ">> vendoring crates"
mkdir -p "$STAGE/.cargo"
# cargo prints the source-replacement stanza on stdout; that is the config.
( cd "$STAGE" && cargo vendor --locked vendor ) > "$STAGE/.cargo/config.toml" 2> "$WORK/vendor.log" \
    || { cat "$WORK/vendor.log" >&2; exit 1; }
grep -q 'replace-with' "$STAGE/.cargo/config.toml" \
    || { echo "error: cargo vendor produced no source replacement" >&2; exit 1; }

FILTER_ARGS=()
for t in "${LINUX_TARGETS[@]}"; do FILTER_ARGS+=(--filter-platform "$t"); done
( cd "$STAGE" && cargo metadata --locked --offline --format-version 1 "${FILTER_ARGS[@]}" ) \
    > "$WORK/metadata.json"

python3 - "$WORK/metadata.json" "$STAGE/vendor" <<'PY'
import json, os, re, shutil, sys, tomllib
meta_path, vendor = sys.argv[1:]
meta = json.load(open(meta_path))
pkgs = {p["id"]: p for p in meta["packages"]}
used = set()
for node in meta["resolve"]["nodes"]:
    p = pkgs[node["id"]]
    if p["source"]:  # registry crates only; the root has no source
        used.add((p["name"], p["version"]))

stubbed = kept = 0
for d in sorted(os.listdir(vendor)):
    path = os.path.join(vendor, d)
    manifest = tomllib.load(open(os.path.join(path, "Cargo.toml"), "rb"))
    pkg = manifest["package"]
    if (pkg["name"], pkg["version"]) in used:
        # dpkg-source silently drops files it treats as editor or VCS debris,
        # *.orig among them — and every crate ships a Cargo.toml.orig. Cargo
        # then refuses the crate over a file its checksum list still names.
        # Remove them here, and from the list, so every distro sees one tree.
        ck_path = os.path.join(path, ".cargo-checksum.json")
        ck = json.load(open(ck_path))
        drop = [f for f in ck["files"]
                if re.search(r"(\.orig|\.rej|~)$|(^|/)\.(git|hg|svn|bzr)", f)]
        for f in drop:
            os.remove(os.path.join(path, f))
            del ck["files"][f]
        if drop:
            with open(ck_path, "w") as fh:
                json.dump(ck, fh)
        kept += 1
        continue
    checksum = json.load(open(os.path.join(path, ".cargo-checksum.json")))
    for entry in os.listdir(path):
        if entry == "Cargo.toml":
            continue
        full = os.path.join(path, entry)
        shutil.rmtree(full) if os.path.isdir(full) else os.remove(full)
    # Every target path the manifest names must exist or loading it fails.
    # They are empty: nothing here is ever compiled on a Linux target.
    lib = manifest.get("lib", {}).get("path", "src/lib.rs")
    paths = {lib}
    build = pkg.get("build")
    if isinstance(build, str):
        paths.add(build)
    for kind in ("bin", "example", "test", "bench"):
        for t in manifest.get(kind, []):
            if "path" in t:
                paths.add(t["path"])
    for rel in paths:
        full = os.path.join(path, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        with open(full, "w") as f:
            f.write("fn main() {}\n" if rel != lib else "")
    with open(os.path.join(path, ".cargo-checksum.json"), "w") as f:
        json.dump({"files": {}, "package": checksum.get("package")}, f)
    stubbed += 1
print(f">> vendor: kept {kept} crates, stubbed {stubbed} not built on Linux")

# The licence of what is actually linked: tapedeck's own plus every crate that
# is built for Linux. Old crates spell alternatives "MIT/Apache-2.0".
exprs, ids = set(), {"AGPL-3.0-only"}
for p in pkgs.values():
    if not p["source"] or (p["name"], p["version"]) not in used or not p.get("license"):
        continue
    e = " OR ".join(x.strip() for x in p["license"].split("/"))
    exprs.add(e)
    # "X WITH Y" is one licence to Arch, so keep it together while splitting.
    flat = e.replace(" WITH ", "\0").replace("(", " ").replace(")", " ")
    ids.update(t.replace("\0", " WITH ") for t in flat.split() if t not in ("OR", "AND"))
def group(e):
    return f"({e})" if " OR " in e or " AND " in e else e
spdx = " AND ".join(["AGPL-3.0-only"] + sorted({group(e) for e in exprs} - {"AGPL-3.0-only"}))
work = os.path.dirname(meta_path)
open(os.path.join(work, "license.spdx"), "w").write(spdx)
open(os.path.join(work, "license.ids"), "w").write(" ".join(f"'{i}'" for i in sorted(ids)))
PY

# The stubs must still resolve exactly as the lockfile says.
( cd "$STAGE" && cargo metadata --locked --offline --format-version 1 "${FILTER_ARGS[@]}" >/dev/null ) \
    || { echo "error: the vendored tree no longer resolves" >&2; exit 1; }

# ── lindera's dictionaries ──────────────────────────────────────────────────
# The `cjk` feature's dictionary crates (lindera-ipadic, lindera-cc-cedict)
# DOWNLOAD their MeCab sources in build.rs, which vendoring does not capture
# and OBS cannot do: v0.121.0 failed on every repository with "Failed to
# download dictionary assets". lindera skips the download when
# LINDERA_BUILD_DICTIONARY_CACHE_DIR holds the archive at
# <dir>/<crate version>-fmt<format version>/<file>, MD5-checked against the
# hash its build.rs pins, so the archives are fetched here and the variable is
# set through [env] in the vendored config, which cargo hands to build scripts
# in every recipe (spec, debian.rules, PKGBUILD) without touching them.
#
# Read from the vendored build.rs rather than written down here, so a lindera
# upgrade brings its new archive along instead of silently missing it.
echo ">> fetching lindera dictionaries"
python3 - "$STAGE" "${XDG_CACHE_HOME:-$HOME/.cache}/tapedeck-obs" <<'PY'
import glob, hashlib, os, re, shutil, sys, tomllib, urllib.request
stage, cache = sys.argv[1:]
vendor = os.path.join(stage, "vendor")
meta_rs = os.path.join(vendor, "lindera-dictionary", "src", "dictionary", "metadata.rs")
fmt = re.search(r"DICTIONARY_FORMAT_VERSION:\s*u32\s*=\s*(\d+)", open(meta_rs).read())
if not fmt:
    sys.exit(f"error: no DICTIONARY_FORMAT_VERSION in {meta_rs}")
fmt = fmt.group(1)

def md5(path):
    h = hashlib.md5()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()

found = 0
for build_rs in sorted(glob.glob(os.path.join(vendor, "lindera-*", "build.rs"))):
    src = open(build_rs).read()
    name = re.search(r'file_name:\s*"([^"]+)"', src)
    if not name:
        continue  # not a dictionary crate, or a stub for another platform
    want = re.search(r'md5_hash:\s*"([0-9a-f]{32})"', src).group(1)
    urls = re.findall(r'"(https?://[^"]+)"', re.search(r"download_urls:\s*&\[(.*?)\]", src, re.S).group(1))
    crate_dir = os.path.dirname(build_rs)
    version = tomllib.load(open(os.path.join(crate_dir, "Cargo.toml"), "rb"))["package"]["version"]
    name = name.group(1)

    # A local copy outside the tarball, so each release does not refetch.
    os.makedirs(cache, exist_ok=True)
    cached = os.path.join(cache, name)
    if not (os.path.exists(cached) and md5(cached) == want):
        for url in urls:
            # lindera.dev answers Python's default User-Agent with a 403.
            req = urllib.request.Request(url, headers={"User-Agent": "tapedeck-obs-source"})
            try:
                with urllib.request.urlopen(req, timeout=120) as r, open(cached + ".part", "wb") as f:
                    shutil.copyfileobj(r, f)
                if md5(cached + ".part") == want:
                    os.replace(cached + ".part", cached)
                    break
                print(f"   {url}: MD5 mismatch", file=sys.stderr)
            except OSError as e:
                print(f"   {url}: {e}", file=sys.stderr)
        else:
            sys.exit(f"error: could not fetch {name} for {os.path.basename(crate_dir)}")
    dest = os.path.join(stage, "lindera-cache", f"{version}-fmt{fmt}")
    os.makedirs(dest, exist_ok=True)
    shutil.copyfile(cached, os.path.join(dest, name))
    print(f"   {os.path.basename(crate_dir)} {version}: {name}")
    found += 1
if found == 0:
    sys.exit("error: no lindera dictionary crate found in vendor/ — has the cjk feature moved?")
PY
cat >> "$STAGE/.cargo/config.toml" <<'EOF'

# lindera's dictionary archives, shipped in the tarball (see make_obs_source.sh).
[env]
LINDERA_BUILD_DICTIONARY_CACHE_DIR = { value = "lindera-cache", relative = true }
EOF

# ── Deterministic tar.gz ────────────────────────────────────────────────────
# Python rather than tar flags: macOS ships bsdtar, whose options differ from
# GNU tar's --sort/--mtime/--owner, and this must give the same bytes anywhere.
echo ">> writing $NAME.tar.gz"
python3 - "$WORK" "$NAME" "$OUT/$NAME.tar.gz" <<'PY'
import gzip, os, sys, tarfile
work, name, dest = sys.argv[1:]
root = os.path.join(work, name)
paths = []
for d, dirs, files in os.walk(root):
    dirs.sort()
    for n in sorted(dirs + files):
        paths.append(os.path.join(d, n))
paths.sort(key=lambda p: os.path.relpath(p, work))
MTIME = 1_700_000_000
with open(dest, "wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as tar:
            def add(p):
                ti = tar.gettarinfo(p, arcname=os.path.relpath(p, work))
                ti.mtime, ti.uid, ti.gid, ti.uname, ti.gname = MTIME, 0, 0, "root", "root"
                if ti.isfile():
                    ti.mode = 0o755 if ti.mode & 0o111 else 0o644
                    with open(p, "rb") as f:
                        tar.addfile(ti, f)
                else:
                    if ti.isdir():
                        ti.mode = 0o755
                    tar.addfile(ti)
            add(root)
            for p in paths:
                add(p)
PY

# One-line change text: the annotated tag's subject, minus a leading
# "vX.Y.Z — " (the entry already says which version), else a stock line.
CHANGE="$(git for-each-ref --format='%(contents:subject)' "refs/tags/$REF" 2>/dev/null || true)"
CHANGE="$(printf '%s' "$CHANGE" | sed -E 's/^(tapedeck[[:space:]]+)?v?[0-9][0-9.]*[[:space:]]*(—|--|-|:)[[:space:]]*//')"
[ -n "$CHANGE" ] || CHANGE="new upstream release."
echo ">> changelog: $CHANGE"

SHA256="$( (sha256sum "$OUT/$NAME.tar.gz" 2>/dev/null || shasum -a 256 "$OUT/$NAME.tar.gz") | cut -d' ' -f1)"
LICENSE="$(cat "$WORK/license.spdx")"
ARCH_LICENSES="$(cat "$WORK/license.ids")"
CHANGES_DATE="$(LC_ALL=C TZ=UTC date '+%a %b %e %H:%M:%S UTC %Y')"
DEB_DATE="$(LC_ALL=C TZ=UTC date '+%a, %d %b %Y %H:%M:%S +0000')"

# The change text is free-form (a tag message), so escape what sed treats as
# special in a replacement: the | delimiter, & and backslash.
CHANGE_SED="$(printf '%s' "$CHANGE" | sed -e 's/[\\|&]/\\&/g')"
fill() {
    sed -e "s|@VERSION@|$VERSION|g" -e "s|@SHA256@|$SHA256|g" \
        -e "s|@LICENSE@|$LICENSE|g" -e "s|@ARCH_LICENSES@|$ARCH_LICENSES|g" \
        -e "s|@CHANGES_DATE@|$CHANGES_DATE|g" -e "s|@DEB_DATE@|$DEB_DATE|g" \
        -e "s|@CHANGE@|$CHANGE_SED|g" \
        "$TPL/$1" > "$OUT/$2"
}
fill tapedeck.spec.in     tapedeck.spec
fill tapedeck.dsc.in      tapedeck.dsc
fill PKGBUILD.in          PKGBUILD
fill tapedeck.changes.in  tapedeck.changes
fill debian.changelog.in  debian.changelog

# Keep the published history under the new entry (see PREV_DIR above).
if [ -n "${PREV_DIR:-}" ]; then
    if grep -q "^tapedeck (${VERSION}-" "$PREV_DIR/debian.changelog" 2>/dev/null; then
        echo ">> $VERSION already in the published changelogs; reusing them unchanged"
        cp "$PREV_DIR/tapedeck.changes" "$PREV_DIR/debian.changelog" "$OUT/"
    else
        for f in tapedeck.changes debian.changelog; do
            [ -s "$PREV_DIR/$f" ] || continue
            { cat "$OUT/$f"; echo; cat "$PREV_DIR/$f"; } > "$OUT/$f.new"
            mv "$OUT/$f.new" "$OUT/$f"
        done
    fi
fi
cp "$TPL/debian.control" "$TPL/debian.rules" "$TPL/debian.copyright" "$OUT/"

echo ">> wrote $OUT"
ls -l "$OUT"
echo ">> sha256 $SHA256  $NAME.tar.gz"
