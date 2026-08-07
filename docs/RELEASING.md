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
5. Commit as `release: <version>`, tag `v<version>`, push with `--tags`.
6. Create a GitHub release from the tag with the notes as the body, attaching platform
   artifacts (.dmg for macOS, AppImage/deb for Linux) once packaging exists.

Versions are semver-shaped but pre-1.0: minor bumps for feature releases, patch bumps
for fix-only releases.
