{
  description = "AppImage bundler";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-23.05";
    flake-utils.url = "github:numtide/flake-utils";

    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };

    appimage-runtime = {
      url = "github:AppImageCrafters/appimage-runtime";
      flake = false;
    };

    squashfuse = {
      # * 53e1b97002c7bc13392a6b0121680d28a94a9715 add support for mounting of subdirectory
      #   which changes API of sqfs_open_image and sqfs_usage
      #   The correct patch should be add NULL when calling them in appimage-runtime
      # * 1d6da796e7d4447af8ce2d7efe51dc053d314221 add low-level uid=N and gid=N options
      url = "github:vasi/squashfuse/0.1.105";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, ... }@inputs:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = (import nixpkgs {
          inherit system;
        }).pkgsStatic;

        # nixpkgs has an old version so we need to package our own
        own-squashfuse = pkgs.stdenv.mkDerivation {
          name = "squashfuse";
          src = inputs.squashfuse;
          nativeBuildInputs = with pkgs; [ autoreconfHook libtool pkg-config ];
          buildInputs = with pkgs; [ lz4 xz zlib lzo zstd fuse ];
        };
      in
      rec {
        packages.apprun = with pkgs; rustPlatform.buildRustPackage rec {
          pname = "app-run";
          version = "0.0.1";

          src = self;
          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          meta = with lib; {
            description = "Run wrapper for AppImage";
            homepage = "https://github.com/KiruyaMomochi/nix-appimage";
            license = licenses.unlicense;
            maintainers = [ maintainers.tailhook ];
          };
        };

        packages.runtime =
          let
            git-commit = "0000000";
          in
          pkgs.runCommandCC "runtime"
            {
              nativeBuildInputs = with pkgs; [ pkg-config ];
              buildInputs = with pkgs; [
                fuse
                own-squashfuse
                zstd
                zlib
                lzma
                lz4
                lzo
              ];
            } ''
            NIX_CFLAGS_COMPILE="$(pkg-config --cflags fuse) $NIX_CFLAGS_COMPILE"

            # extra includes to make things work
            mkdir -p include/squashfuse
            echo "#include_next <squashfuse/squashfuse.h>" > include/squashfuse/squashfuse.h
            cp ${inputs.squashfuse}/fuseprivate.h -t include/squashfuse

            $CC ${inputs.appimage-runtime}/src/main.c -o $out \
              -I./include -D_FILE_OFFSET_BITS=64 -DGIT_COMMIT='"${git-commit}"' \
              -lfuse -lsquashfuse_ll -lzstd -lz -llzma -llz4 -llzo2 \
              -T ${inputs.appimage-runtime}/src/data_sections.ld

            # Add AppImage Type 2 Magic Bytes to runtime
            printf %b '\x41\x49\x02' > magic_bytes
            dd if=magic_bytes of=$out bs=1 count=3 seek=8 conv=notrunc status=none
          '';

        # Produce an (probably non-conforming) AppImage.
        #
        # The AppImage type 2 format is simply the runtime binary concatenated
        # with a squashfs. When running the AppImage, the squashfs binary is
        # extracted/mounted at an arbitrary place and the AppRun binary within
        # run.
        mkappimage = { drv, entrypoint, name }:
          let
            arch = builtins.head (builtins.split "-" system);
            closure = pkgs.writeReferencesToFile drv;
            extras = [
              "AppRun f 555 0 0 cat ${packages.apprun}/bin/app-run"
              "entrypoint s 555 0 0 ${entrypoint}"
              "mountroot d 777 0 0" # TODO permissions?
            ];
            extra-args = pkgs.lib.concatMapStrings (x: " -p \"${x}\"") extras;
          in
          pkgs.runCommand "${name}-${arch}.AppImage"
            {
              nativeBuildInputs = with pkgs; [ squashfsTools ];
            } ''
            mksquashfs $(cat ${closure}) $out \
              -no-strip \
              -offset $(stat -L -c%s ${packages.runtime}) \
              -comp zstd \
              -all-root \
              -noappend \
              -b 1M \
              ${extra-args}
            dd if=${packages.runtime} of=$out conv=notrunc
            chmod 755 $out
          '';

        bundlers.default =
          let
            basename = p: pkgs.lib.lists.last (builtins.split "/" p);

            mainProgram = drv:
              if drv?meta && drv.meta?mainProgram then drv.meta.mainProgram
              else (builtins.parseDrvName (builtins.unsafeDiscardStringContext drv.name)).name;

            program = drv:
              let
                # Use same auto-detect that <https://github.com/NixOS/bundlers>
                # uses. This isn't 100% accurate and might pick the wrong name
                # (e.g. nixpkgs#mesa-demos), so we do an additional check to
                # make sure the target exists
                main =
                  if drv?meta && drv.meta?mainProgram then drv.meta.mainProgram
                  else (builtins.parseDrvName (builtins.unsafeDiscardStringContext drv.name)).name;
                mainPath = "${drv}/bin/${main}";

                # builtins.pathExists mainPath doesn't work consistently (e.g.
                # for symlinks), but this does
                mainPathExists = builtins.hasAttr main (builtins.readDir "${drv}/bin");
              in
              assert pkgs.lib.assertMsg mainPathExists "main program ${mainPath} does not exist";
              mainPath;

            handler = {
              app = drv: mkappimage {
                drv = drv.program;
                entrypoint = drv.program;
                name = basename drv.program;
              };
              derivation = drv: mkappimage {
                drv = drv;
                entrypoint = program drv;
                name = drv.name;
              };
            };
            known-types = builtins.concatStringsSep ", " (builtins.attrNames handler);
          in
          drv:
            assert pkgs.lib.assertMsg (handler ? ${drv.type}) "don't know how to make app image for type '${drv.type}'; only know ${known-types}";
            handler.${drv.type} drv;

        devShell = with nixpkgs.legacyPackages.${system}; mkShell {
          buildInputs = [ cargo rustc rustfmt pre-commit rustPackages.clippy rust-analyzer ];
          RUST_SRC_PATH = rustPlatform.rustLibSrc;
        };
      });
}
