{
  description = "Foilfern Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    # Match scripts/cloud-setup.sh and the gitlinks at this upstream revision.
    # Fetch only the submodules used by the Rust build and reference tests.
    upstream = {
      url = "github:LibrePCB/LibrePCB/7f1b548abb61bd59b2b9c1166eca3e6557480c17";
      flake = false;
    };
    upstream-data = {
      url = "github:LibrePCB/librepcb-test-data/b0724e58b6755354d93c60f3ed3afdf109069927";
      flake = false;
    };
    upstream-fonts = {
      url = "github:LibrePCB/fontobene-fonts/f197bc5157bec6770af06601894ee4d40d93796a";
      flake = false;
    };
    upstream-i18n = {
      url = "github:LibrePCB/librepcb-i18n/ad2aa345e8030393bc166249236b42f388a9149e";
      flake = false;
    };
    upstream-fontobene-qt = {
      url = "github:fontobene/fontobene-qt/c3404d0b56327258120a4beb121101d6706296e6";
      flake = false;
    };
  };

  outputs =
    inputs@{ nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
    in
    {
      devShells = nixpkgs.lib.genAttrs systems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          upstreamSource = pkgs.runCommand "librepcb-reference-source" { } ''
            mkdir -p "$out"
            cp -rs --no-preserve=mode ${inputs.upstream}/. "$out/"
            rm -rf "$out/tests/data" "$out/share/librepcb/fontobene" "$out/i18n" "$out/libs/fontobene-qt"
            ln -s ${inputs.upstream-data} "$out/tests/data"
            ln -s ${inputs.upstream-fonts} "$out/share/librepcb/fontobene"
            ln -s ${inputs.upstream-i18n} "$out/i18n"
            ln -s ${inputs.upstream-fontobene-qt} "$out/libs/fontobene-qt"
          '';
          libraries = with pkgs; [
            fontconfig
            freetype
            libxkbcommon
            wayland
            libGL
            vulkan-loader
            libX11
            libXcursor
            libXi
            libXrandr
            libxcb
            libXext
            libXfixes
            udev
            stdenv.cc.cc.lib
          ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer
              pkg-config
              cmake
              ninja
              git
              uv
              nixfmt
            ];
            buildInputs = libraries;
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
            shellHook = ''
              export LIBREPCB_UPSTREAM_DIR="''${LIBREPCB_UPSTREAM_DIR:-${upstreamSource}}"
              export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath libraries}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
            '';
          };
        }
      );
      formatter = nixpkgs.lib.genAttrs systems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
