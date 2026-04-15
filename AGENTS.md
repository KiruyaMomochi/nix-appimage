# nix-appimage

Fork of [ralismark/nix-appimage](https://github.com/ralismark/nix-appimage) with a Rust AppRun replacing the original C implementation. Primary use case is bundling `hwspec` to run on unsupported machines.

## Architecture

The AppImage execution flow: **runtime** (FUSE mount squashfs) → **AppRun** (Rust binary: namespace + bind mount + chroot) → **entrypoint**.

- `src/main.rs` — AppRun: creates mount/user namespaces, bind mounts host filesystem into a tmpfs, overlays the bundled /nix/store, chroots, then execves the entrypoint.
- `src/id_map.rs` — uid/gid map helpers for user namespace setup.
- `mkAppImage.nix` — Nix derivation that assembles the squashfs and prepends the runtime.
- `flake.nix` — Exposes `mkAppImage`, runtimes, appruns, and the `nix bundle` interface.
- `extra-files.sh` — Best-effort extraction of .desktop files and icons from the bundled derivation.

## Build

This project builds via Nix (`pkgsStatic` for static linking). The Rust binary is built through `rustPlatform.buildRustPackage` in `appruns/userns-chroot/default.nix`.

For local iteration, `cargo check`/`cargo build` works with a recent Rust toolchain. The project-local `.cargo/config.toml` points at an internal crates mirror — if that's down, override or remove it.

## Design Decisions

- **Environment is intentionally wiped** in `execve` (only `TERM` is passed). This is by design since the bundled app is a CLI info-collection tool, not a GUI app.
- **`--apprun-*` flag prefix**: CLI args starting with `--apprun-` are consumed by AppRun (with the prefix stripped); everything else is passed through to the entrypoint. This avoids flag conflicts.
- **Host /nix/store merging** (`mount_nix`): When the host has `/nix`, store paths not present in the squashfs are bind-mounted in. This allows the AppImage to access additional nix store paths at runtime.

## Known Issues

- **Ubuntu 23.10+**: Unprivileged user namespaces are restricted by AppArmor. Users need `sudo sysctl kernel.apparmor_restrict_unprivileged_userns=0` or an AppArmor profile exception.
- **OpenGL on non-NixOS**: Known upstream limitation, see nixpkgs#9415. Not in scope; use nixGL if needed.
