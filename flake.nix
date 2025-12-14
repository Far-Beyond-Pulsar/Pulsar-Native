{
  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
    naersk.url = "github:nix-community/naersk";
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs =
    {
      self,
      flake-utils,
      naersk,
      nixpkgs,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = (import nixpkgs) {
          inherit system;
          overlays = [
            (import rust-overlay)
          ];
        };

        naersk' = pkgs.callPackage naersk { };

        rustToolchain = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [
            "rust-src"
            "cargo"
            "rustc"
            "clippy"
            "rustfmt"
          ];
        };

        buildInputs = with pkgs; [
        ];

        nativeBuildInputs =
          with pkgs;
          [
            rustToolchain

            pkg-config
            protobuf
            openssl
          ]
          # TODO(mdeand): Add support for macOS 
          ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            libxkbcommon
            wayland
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            xorg.libXrandr
            alsa-lib
          ];
      in
      rec {
        defaultPackage = packages.engine;
        packages = {
          engine = naersk'.buildPackage {
            src = ./.;
            nativeBuildInputs = nativeBuildInputs;
            buildInputs = buildInputs;
          };
          container = pkgs.dockerTools.buildImage {
            name = "engine";
            config = {
              entrypoint = [ "${packages.engine}/bin/engine" ];
            };
          };
        };

        devShell = pkgs.mkShell {
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          nativeBuildInputs =
            with pkgs;
            [
              nixfmt
              cmake
              rust-analyzer
            ]
            ++ buildInputs
            ++ nativeBuildInputs;
        };
      }
    );
}
