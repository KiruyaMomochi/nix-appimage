{ rustPlatform
, lib
, ...
}:
rustPlatform.buildRustPackage rec {
  pname = "app-run";
  version = "0.0.1";

  src = ../../.;
  cargoLock = {
    lockFile = ../../Cargo.lock;
  };

  postInstall = ''
    mv $out/bin/app-run $out/AppRun
    rmdir $out/bin
    mkdir $out/mountroot
  '';

  meta = with lib; {
    description = "Run wrapper for AppImage";
    homepage = "https://github.com/KiruyaMomochi/nix-appimage";
    license = licenses.unlicense;
    maintainers = [ maintainers.tailhook ];
  };
}
