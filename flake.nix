{
  description = "A basic AppImage bundler";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-24.11";
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

  outputs = { self, nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = (import nixpkgs { inherit system; }).pkgsStatic;
      in
      rec {
        # runtimes are an executable that mount the squashfs part of the appimage and start AppRun
        packages.appimage-runtimes = {
          appimagecrafters = pkgs.callPackage ./runtimes/appimagecrafters { };
          appimage-type2-runtime = pkgs.callPackage ./runtimes/appimage-type2-runtime { };
        };

        # appruns contain an AppRun executable that does setup and launches entrypoint
        packages.appimage-appruns = {
          userns-chroot = pkgs.callPackage ./appruns/userns-chroot { };
        };

        lib.mkAppImage = pkgs.callPackage ./mkAppImage.nix {
          mkappimage-runtime = packages.appimage-runtimes.appimage-type2-runtime;
          mkappimage-apprun = packages.appimage-appruns.userns-chroot;
        };

        bundlers.default = drv:
          if drv.type == "app" then
            lib.mkAppImage
              {
                program = drv.program;
              }
          else if drv.type == "derivation" then
            lib.mkAppImage
              {
                program = pkgs.lib.getExe drv;
              }
          else builtins.abort "don't know how to build ${drv.type}; only know app and derivation";

        checks =
          let
            # use regular (non-static) nixpkgs
            pkgs = import nixpkgs { inherit system; };
            hello-appimage = bundlers.default pkgs.hello;
          in
          {
            hello-is-static = pkgs.runCommand "check-hello-is-static"
              {
                nativeBuildInputs = [ (pkgs.lib.getBin pkgs.stdenv.cc.libc) ];
              } ''
              (! ldd ${hello-appimage} 2>&1) | grep "not a dynamic executable"
              touch $out
            '';
          };
      });
}
