{ lib, rustPlatform }:
rustPlatform.buildRustPackage {
  pname = "temporalShell";
  version = "0.1.0";
  src = lib.cleanSourceWith {
    src = ./.;
    filter = path: type: baseNameOf path != "target" && baseNameOf path != "result";
  };
  cargoLock.lockFile = ./Cargo.lock;
  meta = {
    description = "Capability-based Wayland border";
    license = lib.licenses.mit;
    mainProgram = "temporalShell";
  };
}
