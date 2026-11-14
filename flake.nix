{
  description = "Pulsar Native engine";

  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = inputs:
    let
      systems = [
        "x86_64-linux"
        "i686-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      eachSystem = inputs.nixpkgs.lib.genAttrs systems;

      buildFor = system:
        let
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ (import inputs.rust-overlay) ];
          };
          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rustfmt" "clippy" ];
          };
          runtimeLibraries = with pkgs; [
            fontconfig
            glib
            gdk-pixbuf
            gtk3
            openssl
            vulkan-loader
            mesa
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            wayland
            wayland-protocols
            libxkbcommon
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
            xorg.libxcb
            xorg.libXrender
            xorg.libXfixes
            alsa-lib
          ];
          buildTools = with pkgs; [
            rustToolchain
            cmake
            pkg-config
            protobuf
          ];
          engine = (pkgs.callPackage inputs.naersk { }).buildPackage {
            src = inputs.self;
            nativeBuildInputs = buildTools;
            buildInputs = runtimeLibraries;
          };
          container = pkgs.dockerTools.buildImage {
            name = "engine";
            config.entrypoint = [ "${engine}/bin/engine" ];
          };
          shell = pkgs.mkShell {
            packages = buildTools ++ [ pkgs.nixfmt pkgs.rust-analyzer ];
            buildInputs = runtimeLibraries;
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibraries;
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          };
        in
        {
          packages = {
            default = engine;
            inherit engine container;
          };
          devShells.default = shell;
          defaultPackage = engine;
        };
    in
    {
      packages = eachSystem (system: (buildFor system).packages);
      devShells = eachSystem (system: (buildFor system).devShells);
      defaultPackage = eachSystem (system: (buildFor system).defaultPackage);
    };
}
