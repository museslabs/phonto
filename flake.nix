{
  description = "GPU-accelerated video wallpaper for Wayland compositors and macOS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        cargoToml = (pkgs.lib.importTOML ./Cargo.toml).package;

        gst_plugins = with pkgs.gst_all_1; [
          gstreamer
          gst-plugins-base
          gst-plugins-good
          gst-plugins-bad
          gst-plugins-ugly
          gst-libav
        ];

        linuxDeps =
          with pkgs;
          [
            wayland
            libGL
            mesa
          ]
          ++ gst_plugins;

        rustToolchain = pkgs.rust-bin.stable.latest.default;
      in
      {
        packages = {
          default = self.packages.${system}.phonto;

          phonto = pkgs.rustPlatform.buildRustPackage {
            pname = cargoToml.name;
            version = cargoToml.version;

            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              rustToolchain
              pkg-config
              wrapGAppsHook4
            ];

            buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux linuxDeps;

            dontWrapGApps = !pkgs.stdenv.isLinux;

            meta = with pkgs.lib; {
              description = cargoToml.description;
              homepage = "https://github.com/museslabs/phonto";
              license = licenses.gpl3Plus;
              platforms = platforms.linux ++ platforms.darwin;
              maintainers = with lib.maintainers; [ lonerOrz ];
              mainProgram = "phonto";
            };
          };
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            rustToolchain
            pkgs.pkg-config
            pkgs.gst_all_1.gstreamer
          ];

          buildInputs = pkgs.lib.optionals pkgs.stdenv.isLinux linuxDeps;

          shellHook = pkgs.lib.optionalString pkgs.stdenv.isLinux ''
            export GST_PLUGIN_PATH_1_0="${pkgs.lib.makeSearchPath "lib/gstreamer-1.0" gst_plugins}"
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath linuxDeps}:$LD_LIBRARY_PATH"
          '';
        };
      }
    );
}
