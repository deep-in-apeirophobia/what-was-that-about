{
  description = "What Was That About - dev shell (NixOS / nix users only; other platforms see README)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
    in {
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            cmake
            clang
            rustPlatform.bindgenHook
            shaderc # glslc, needed by whisper.cpp's Vulkan backend
            nodejs_22
            pnpm
          ];
          buildInputs = with pkgs; [
            webkitgtk_4_1
            gtk3
            libsoup_3
            glib
            openssl
            librsvg
            vulkan-headers
            vulkan-loader
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            gst_all_1.gst-plugins-good
            gst_all_1.gst-plugins-bad
            gst_all_1.gst-libav
          ];
          shellHook = ''
            # /run/opengl-driver/lib: NixOS GPU drivers (NVIDIA's Vulkan ICD is found by soname)
            export LD_LIBRARY_PATH=/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader pkgs.libGL ]}:''${LD_LIBRARY_PATH:-}
            export WEBKIT_DISABLE_DMABUF_RENDERER=1
          '';
        };
      });
    };
}
