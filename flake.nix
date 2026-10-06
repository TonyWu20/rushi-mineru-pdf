# rushi-mineru-pdf — ext flake (Mode B, ext-flake-authoring.md §3).
#
# Exposes one rushi tool package:
#   packages.<system>.mineru   mineru  (MinerU pdf/office-to-markdown/latex)
#
# $out contracts (ext-flake-authoring.md §2):
#   tool: $out/mineru/tool.toml + $out/mineru/bin/mineru
#
# The tool shells out to curl (API calls) and unzip (result archive).
# Both are bundled next to the tool binary ($out/mineru/bin/), the same
# pattern as rushi-web-access bundling obscura. The tool looks for them
# in its own exe dir before PATH, so the Nix-built package is
# self-contained.
#
# The crate depends only on serde_json; its lock file pins the same
# serde / serde_json versions as rushi-web-access, so the cargo
# vendor dir on the store already carries the dependencies and the
# sandboxed build needs no new crate fetch.
#
# Usage from a consumer flake (ext-flake-authoring.md §5):
#   inputs.rushi-mineru-pdf.url = "github:TonyWu20/rushi-mineru-pdf?ref=<rev>";
#   rushi.external_tools = [ mineruFlake.packages.${system}.mineru ];

{
  description = "rushi-mineru-pdf — MinerU pdf/office-to-markdown/latex tool for rushi";

  inputs = {
    # Pinned to the same nixpkgs/fenix revs as the other rushi ext
    # repos, so the toolchain builds are shared on the store.
    nixpkgs.url = "github:NixOS/nixpkgs/8ce4ef6cb6f871616146b9fe26d2a5ae594e94fe";
    fenix = {
      url = "github:nix-community/fenix/17757a67ed6621e70efc4271258825f34ee8409e";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, fenix }:
    let
      # Mirror the kernel flake: fixed system list + genAttrs.
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      pkgLib = nixpkgs.lib;

      buildFor = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "clippy"
            "rust-src"
            "rustc"
            "rustfmt"
            "rust-analyzer"
          ];

          # Build the single cargo crate from a subpath of this flake's
          # source tree. The whole repo is the source (ext-flake-
          # authoring.md §3 note); buildAndTestSubdir cd's into the
          # crate before cargo runs.
          buildCrate = pkgs.rustPlatform.buildRustPackage {
            pname = "mineru";
            version = "0.1.0";
            src = "${self}";
            buildAndTestSubdir = "mineru";
            cargoRoot = "mineru";
            nativeBuildInputs = [ rustToolchain ];
            cargoLock = { lockFile = "${self}/mineru/Cargo.lock"; };
            doCheck = false;
          };

          # Tool wrapper: $out/mineru/tool.toml + mineru/bin/mineru.
          # curl and unzip are bundled into the same bin/ dir so the
          # tool binary (which looks for them next to itself first)
          # is self-contained in the Nix-built package.
          mineruPkg = pkgs.stdenv.mkDerivation {
            pname = "mineru-tool";
            version = "0.1.0";
            # this derivation only copies files; stdenv still wants a
            # source for unpackPhase, so use a placeholder
            src = pkgs.writeTextFile {
              name = "mineru-tool-src";
              destination = "/placeholder";
              text = "";
            };
            nativeBuildInputs = [ buildCrate pkgs.curl pkgs.unzip ];
            installPhase = ''
              mkdir -p $out/mineru/bin
              cp -rL ${buildCrate}/bin/. $out/mineru/bin/
              cp ${pkgs.curl}/bin/curl $out/mineru/bin/curl
              cp ${pkgs.unzip}/bin/unzip $out/mineru/bin/unzip
              cp ${self}/mineru/tool.toml $out/mineru/tool.toml
            '';
            # meta.rushi.entry (rushi#13): the tool entry dir this
            # package provides. lib.mkRushi reads it at eval time to
            # derive the consumer's [paths] extension_tool_paths
            # (declare-once).
            meta = {
              description = "mineru tool for rushi";
              rushi = { entry = "mineru"; };
            };
          };
        in
        {
          mineru = mineruPkg;
          default = mineruPkg;
        };
    in
    {
      # Standard flake shape: packages.<system>.<name>. `nix build .`
      # resolves packages.<host>.default, and consumers write
      # extFlake.packages.${system}.mineru.
      packages = pkgLib.genAttrs supportedSystems (system: buildFor system);

      # Dev shell: in-tree cargo dev with curl + unzip on PATH.
      devShells = pkgLib.genAttrs supportedSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "rust-src"
            "rustc"
            "rust-analyzer"
          ];
        in
        {
          default = pkgs.mkShell {
            buildInputs = [ rustToolchain pkgs.curl pkgs.unzip ];
            shellHook = ''
              echo "rushi-mineru-pdf dev shell: rust + curl + unzip on PATH."
              echo "Build: cargo build --release --manifest-path mineru/Cargo.toml"
            '';
          };
        }
      );
    };
}
