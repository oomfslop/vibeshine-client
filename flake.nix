{
  description = "Moonlight Qt with Apollo host extensions";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        # Reuse the nixpkgs derivation (qmake + pkg-config, CONFIG+=disable-prebuilts)
        # but build this tree, including the modified moonlight-common-c submodule.
        moonlight-qt = pkgs.moonlight-qt.overrideAttrs (old: {
          version = "6.1.0-apollo";
          src = ./.;
          # Upstream master already contains the fix that nixpkgs patches in
          patches = [ ];
          # The FP16 Metal shaders need the simd half types from the macOS 15 SDK;
          # nixpkgs' default darwin SDK is still 14.4.
          buildInputs = old.buildInputs
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk_15 ];
          # Upstream's MoltenVK HDR path needs a libplacebo newer than nixpkgs has
          # (PL_COLOR_TRC_SCRGB). Metal is the primary renderer on macOS anyway, so
          # build without the Vulkan renderer there.
          qmakeFlags = old.qmakeFlags
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ "CONFIG+=disable-libplacebo" ];
        });
        # The GPUI launcher. Built with cargoLock so no vendor hash is needed.
        sloplight = pkgs.rustPlatform.buildRustPackage {
          pname = "sloplight";
          version = "0.1.0";
          src = ./sloplight;
          cargoLock.lockFile = ./sloplight/Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config pkgs.makeWrapper ] ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.cmake ];
          buildInputs = pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.apple-sdk_15 ]
            ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.fontconfig pkgs.freetype pkgs.libxkbcommon pkgs.wayland pkgs.xorg.libX11 pkgs.xorg.libxcb pkgs.vulkan-loader ];
          # Point the launcher at the streaming helper from this flake
          postInstall = ''
            wrapProgram $out/bin/sloplight --set-default SLOPLIGHT_HELPER ${moonlight-qt}/bin/moonlight
          '';
          doCheck = false;
        };
        default = moonlight-qt;
      });

      apps = forAll (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.system}.default}/bin/moonlight";
        };
        sloplight = {
          type = "app";
          program = "${self.packages.${pkgs.system}.sloplight}/bin/sloplight";
        };
      });

      # Incremental development builds:
      #   nix develop
      #   qmake6 moonlight-qt.pro CONFIG+=disable-prebuilts && make -j$(nproc)
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.system}.default ];
          packages = [ pkgs.qt6.qttools pkgs.cargo pkgs.rustc pkgs.rust-analyzer ];
          shellHook = ''
            export QT_PLUGIN_PATH=${pkgs.qt6.qtbase}/${pkgs.qt6.qtbase.qtPluginPrefix}:${pkgs.qt6.qtsvg}/${pkgs.qt6.qtbase.qtPluginPrefix}:${pkgs.qt6.qtdeclarative}/${pkgs.qt6.qtbase.qtPluginPrefix}
            export QML2_IMPORT_PATH=${pkgs.qt6.qtdeclarative}/${pkgs.qt6.qtbase.qtQmlPrefix}
          '';
        };
      });
    };
}
