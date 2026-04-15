{
  fetchFromGitHub,
  stdenv,
  fuse3,
  pkg-config,
  squashfuse,
  zstd,
  zlib,
  xz,
  lz4,
  lzo,
}:

let
  src = fetchFromGitHub {
    owner = "AppImage";
    repo = "type2-runtime";
    rev = "01164bfcbc8dd2bd0d7e3706f97035108d6b91ba";
    hash = "sha256-GR3LMuWMSafQmc2RQyveue3sq+HYBtl+VkcZVYMS0CI=";
  };

  # Undo nixpkgs' NixOS-specific path hardcoding so the AppImage works on any Linux.
  # nixpkgs replaces /bin/mount and /bin/umount with nix store paths, and sets
  # FUSERMOUNT_DIR to /run/wrappers/bin — none of which exist on non-NixOS targets.
  fuse3' = fuse3.overrideAttrs (old: {
    preConfigure = ''
      # Only substitute /bin/sh (harmless); do NOT replace /bin/mount or /bin/umount
      # with nix store paths — those won't exist on the target machine.
      substituteInPlace util/mount.fuse.c \
        --replace-fail "/bin/sh" "${stdenv.shell}"
    '';
    env = (old.env or { }) // {
      # Point fusermount lookup at /usr/bin (standard on Debian/Ubuntu/Fedora/Arch).
      # The comment in nixpkgs says it "falls back to calling fusermount in $PATH",
      # but that fallback was removed in fuse3. We need the dir to be correct.
      NIX_CFLAGS_COMPILE = ''-DFUSERMOUNT_DIR="/usr/bin"'';
    };
  });

  squashfuse' =
    (squashfuse.override {
      fuse3 = fuse3';
    }).overrideAttrs
      (old: {
        postInstall = (old.postInstall or "") + ''
          cp *.h -t $out/include/squashfuse/
        '';
      });
in
stdenv.mkDerivation {
  pname = "appimage-type2-runtime";
  version = "unstable-2024-08-17";

  inherit src;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [
    fuse3'
    squashfuse'
    zstd
    zlib
    xz
    lz4
    lzo
  ];

  patchPhase = ''
    sed -e '/sqfs_usage/s/);/, true\0/' -i src/runtime/runtime.c
  '';

  configurePhase = ''
    $PKG_CONFIG --cflags fuse3 > cflags
  '';

  buildPhase = ''
    $CC src/runtime/runtime.c -o $out \
      -D_FILE_OFFSET_BITS=64 -DGIT_COMMIT='"0000000"' \
      $(cat cflags) \
      -std=gnu99 -Os -ffunction-sections -fdata-sections -Wl,--gc-sections -static -Wall -Werror \
      -lsquashfuse -lsquashfuse_ll -lfuse3 -lzstd -lz -llzma -llz4 -llzo2 \
      -T src/runtime/data_sections.ld

    # Add AppImage Type 2 Magic Bytes to runtime
    printf %b '\x41\x49\x02' > magic_bytes
    dd if=magic_bytes of=$out bs=1 count=3 seek=8 conv=notrunc status=none
  '';

  dontFixup = true;
}
