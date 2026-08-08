# Releasing Kairn

The release unit is the whole workspace: the app, the `kairn` CLI, and kairn-core share
one version, set once in the root `Cargo.toml` under `[workspace.package]`. The vendored
gpui-terminal fork is versioned independently and is not released on its own.

## During development

User-facing changes are drafted in `release-notes/UNRELEASED.md` as they land, following
the copy rules at the top of that file. Commits are the raw ledger; the unreleased file
is the curated one.

## Cutting a release

1. Curate `release-notes/UNRELEASED.md`: rewrite to the copy rules, drop empty sections.
2. Bump `version` in the root `Cargo.toml` (`[workspace.package]`) and run
   `cargo build` so `Cargo.lock` picks it up.
3. Move the curated notes to `release-notes/<version>.md` and reset `UNRELEASED.md` to
   its empty template.
4. Prepend the same notes to `CHANGELOG.md` under a `## <version> (YYYY-MM-DD)` heading.
5. Build the artifacts (see Packaging below).
6. Commit as `release: <version>`, tag `v<version>`, push with `--tags`.
7. Create a GitHub release from the tag with the notes as the body, attaching the
   `.dmg`, `.AppImage`, and `.deb`.

Versions are semver-shaped but pre-1.0: minor bumps for feature releases, patch bumps
for fix-only releases.

## Packaging

Artifacts are built with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager)
(`cargo install cargo-packager`); config lives in `crates/kairn-app/Cargo.toml` under
`[package.metadata.packager]`. Each build first runs `scripts/prep-binaries.sh`, which on
macOS produces a universal (Apple Silicon + Intel) binary and on Linux a native one.

**macOS** — `./scripts/package-macos.sh` builds `dist/Kairn-<version>.dmg`, signed with a
Developer ID identity and notarized + stapled. It needs a `Developer ID Application`
certificate and a notarytool keychain profile (override `SIGN_IDENTITY` / `NOTARY_PROFILE`,
or run `SIGN=0 NOTARIZE=0 ./scripts/package-macos.sh` to build an unsigned bundle). Verify
with `spctl -a -vvv dist/Kairn.app` and `xcrun stapler validate dist/Kairn-<version>.dmg`.

**Linux** — run `./scripts/package-linux.sh` on a Linux machine (no cross-compiling from
macOS) to build the `.deb` and `.AppImage` into `dist/`. No signing is involved. The build
needs the dev headers listed in the README.
