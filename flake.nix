{
  description = "tpdf: terminal PDF viewer and development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
    in {
      packages = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs { inherit system; };
          manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = manifest.package.name;
            version = manifest.package.version;
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./LICENSE ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postInstall = ''
              wrapProgram "$out/bin/tpdf" \
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [ pkgs.pdfium ]}
            '';
            meta = {
              description = manifest.package.description;
              mainProgram = "tpdf";
              license = pkgs.lib.licenses.mit;
              platforms = pkgs.lib.platforms.linux;
            };
          };
        });

      apps = nixpkgs.lib.genAttrs systems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/tpdf";
          meta.description = "Terminal PDF viewer with PDFium";
        };
      });

      devShells = nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer pkg-config typst ];
            buildInputs = with pkgs; [ pdfium ];
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [ pdfium ]);
            shellHook = ''
              export CARGO_HOME="$PWD/.cargo-home"
            '';
          };
        });
    };
}
