{
  description = "Scarlet OS kernel development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f system);
    in
    {
      devShells = forAllSystems (
        system:
        let
          edk2-202502-overlay = final: prev: {
            edk2 = prev.edk2.overrideAttrs (finalAttrs: prevAttrs: {
              version = "202502";
              srcWithVendoring = prev.fetchFromGitHub {
                owner = "tianocore";
                repo = "edk2";
                tag = "edk2-stable${finalAttrs.version}";
                fetchSubmodules = true;
                hash = "sha256-iobC0CeWSylS9sLuXOqAmL36hl/tY+IedT/I3xQ80Ag=";
              };
              src = prev.applyPatches {
                name = "edk2-${finalAttrs.version}-unvendored-src";
                src = finalAttrs.srcWithVendoring;
                patches = [
                  (prev.fetchpatch {
                    url = "https://src.fedoraproject.org/rpms/edk2/raw/08f2354cd280b4ce5a7888aa85cf520e042955c3/f/0021-Tweak-the-tools_def-to-support-cross-compiling.patch";
                    hash = "sha256-E1/fiFNVx0aB1kOej2DJ2DlBIs9tAAcxoedym2Zhjxw=";
                  })
                ];
                postPatch = ''
                  substituteInPlace BaseTools/Conf/tools_def.template --replace-fail \
                    'DEFINE CLANGPDB_WARNING_OVERRIDES    = ' \
                    'DEFINE CLANGPDB_WARNING_OVERRIDES    = -Wno-unneeded-internal-declaration '
                '';
              };
            });
          };

          overlays = [
            edk2-202502-overlay
            (import rust-overlay)
          ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };

          rustToolchain = pkgs.rust-bin.nightly."2025-12-31".default.override {
            extensions = [ "rust-src" "llvm-tools-preview" ];
            targets = [
              "riscv64gc-unknown-none-elf"
              "aarch64-unknown-none"
            ];
          };

          rustHostTriple =
            {
              x86_64-linux = "x86_64-unknown-linux-gnu";
              aarch64-linux = "aarch64-unknown-linux-gnu";
              x86_64-darwin = "x86_64-apple-darwin";
              aarch64-darwin = "aarch64-apple-darwin";
            }
            .${system};

          rustHostTripleEnv = builtins.replaceStrings [ "-" ] [ "_" ] rustHostTriple;

          scarletRustSrc = pkgs.fetchFromGitHub {
            owner = "petitstrawberry";
            repo = "rust";
            rev = "b9573d6cd0731d24486f77ddf24d502e2e6bef02";
            hash = "sha256-KvC4l6i6LJ91acSJDZOklmASWYB6LAAY2QHGBVU4r0c=";
            fetchSubmodules = true;
          };

          scarletRustCargoDeps = pkgs.rustPlatform.fetchCargoVendor {
            name = "scarlet-rust-cargo-deps";
            src = scarletRustSrc;
            hash = "sha256-n9gzpaUkXa+8Yr8SqtRj0JD9YyVMXtOvk5NxLWrHJsA=";
          };

          scarletRustBootstrapCargoDeps = pkgs.rustPlatform.fetchCargoVendor {
            name = "scarlet-rust-bootstrap-cargo-deps";
            src = scarletRustSrc;
            cargoRoot = "src/bootstrap";
            hash = "sha256-xxaYOc4Xn3F3ghDEpFR2041gJ98t4lONJ9Ka1OkzGzI=";
          };

          scarletRustLibraryCargoDeps = pkgs.rustPlatform.fetchCargoVendor {
            name = "scarlet-rust-library-cargo-deps";
            src = scarletRustSrc;
            cargoRoot = "library";
            hash = "sha256-8XV6J9hMZ9cQ1I6GxEtF3WaOeqaE+WkMTc67mV6O60g=";
          };

          scarletRustToolchain = pkgs.stdenv.mkDerivation {
            pname = "scarlet-rust-toolchain";
            version = "b9573d6";
            src = scarletRustSrc;

            nativeBuildInputs = [
              rustToolchain
              pkgs.cmake
              pkgs.git
              pkgs.ninja
              pkgs.pkg-config
              pkgs.python3
              pkgs.rsync
            ];

            buildInputs = [
              pkgs.zlib
            ];

            dontConfigure = true;

            buildPhase = ''
              runHook preBuild

              export HOME="$TMPDIR"
              export CARGO_HOME="$TMPDIR/cargo"
              export CXX_${rustHostTripleEnv}="${if pkgs.stdenv.isDarwin then "clang++" else "c++"}"

              mkdir -p "$TMPDIR/vendor-registry" "$CARGO_HOME"
              cp -R ${scarletRustCargoDeps}/source-registry-0/. "$TMPDIR/vendor-registry/"
              chmod -R u+w "$TMPDIR/vendor-registry"
              cp -R ${scarletRustBootstrapCargoDeps}/source-registry-0/. "$TMPDIR/vendor-registry/"
              chmod -R u+w "$TMPDIR/vendor-registry"
              cp -R ${scarletRustLibraryCargoDeps}/source-registry-0/. "$TMPDIR/vendor-registry/"
              chmod -R u+w "$TMPDIR/vendor-registry"
              mkdir -p .cargo
              cat > .cargo/config.toml <<EOF
              [source.crates-io]
              replace-with = "vendored-sources"

              [source.vendored-sources]
              directory = "$TMPDIR/vendor-registry"

              [net]
              offline = true
              EOF

              cat > bootstrap.toml <<EOF
              change-id = "ignore"
              profile = "compiler"

              [build]
              build = "${rustHostTriple}"
              host = ["${rustHostTriple}"]
              target = ["${rustHostTriple}", "riscv64gc-unknown-scarlet", "aarch64-unknown-scarlet"]
              cargo = "${rustToolchain}/bin/cargo"
              rustc = "${rustToolchain}/bin/rustc"
              rustfmt = "${rustToolchain}/bin/rustfmt"
              docs = false
              submodules = false

              [llvm]
              download-ci-llvm = false
              targets = "AArch64;RISCV;X86"

              [rust]
              download-rustc = false
              EOF

              ./x build --config bootstrap.toml compiler/rustc library

              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall

              mkdir -p "$out"
              cp -R "build/${rustHostTriple}/stage1/." "$out/"
              ln -sfn ${rustToolchain}/bin/cargo "$out/bin/cargo"
              ln -sfn ${rustToolchain}/bin/cargo-clippy "$out/bin/cargo-clippy"
              ln -sfn ${rustToolchain}/bin/cargo-fmt "$out/bin/cargo-fmt"
              ln -sfn ${rustToolchain}/bin/clippy-driver "$out/bin/clippy-driver"
              ln -sfn ${rustToolchain}/bin/rustfmt "$out/bin/rustfmt"
              ln -sfn ${scarletRustSrc} "$out/lib/rustlib/rustc-src/rust"
              ln -sfn ${scarletRustSrc} "$out/lib/rustlib/src/rust"
              "$out/bin/rustc" --print target-list | grep -E '^(riscv64gc|aarch64)-unknown-scarlet$'

              runHook postInstall
            '';
          };

          # Build UEFI firmware from edk2 source, cross-compiled via GCC5.
          # Patch out -Werror in the template before edksetup.sh generates tools_def.txt.
          buildOvmf =
            { crossPkgs, dscPath, pname }:
            (crossPkgs.edk2.mkDerivation dscPath {
              inherit pname;
              inherit (pkgs.edk2) version;
              hardeningDisable = [
                "format"
                "fortify"
              ];
              nativeBuildInputs = [ pkgs.acpica-tools ];
            }).overrideAttrs (finalAttrs: prevAttrs: {
              prePatch =
                prevAttrs.prePatch
                + ''
                  # Copy BaseTools to a writable location and patch -Werror
                  BT_SRC=$(readlink -f BaseTools)
                  rm -f BaseTools
                  cp -r "$BT_SRC" BaseTools
                  chmod -R u+w BaseTools
                  substituteInPlace BaseTools/Conf/tools_def.template \
                    --replace-fail "-Werror" "-Wno-error"
                '';
            });

          ovmf-riscv64 = buildOvmf {
            crossPkgs = pkgs.pkgsCross.riscv64;
            dscPath = "OvmfPkg/RiscVVirt/RiscVVirtQemu.dsc";
            pname = "OVMF-riscv64";
          };

          padFirmware =
            {
              name,
              src,
              size,
              files,
            }:
            pkgs.runCommand name { } ''
              mkdir -p "$out/FV"
              cp -r ${src}/FV/. "$out/FV/"

              pad_file() {
                local file="$1"
                local size="$2"
                local current

                current="$(${pkgs.coreutils}/bin/stat -c %s "$file")"
                if [ "$current" -gt "$size" ]; then
                  echo "$file is larger than requested pflash size $size" >&2
                  exit 1
                fi

                chmod u+w "$file"
                if [ "$current" -lt "$size" ]; then
                  dd if=/dev/zero bs=1 count="$((size - current))" status=none | tr '\000' '\377' >> "$file"
                fi
              }

              ${pkgs.lib.concatMapStringsSep "\n" (file: ''pad_file "$out/FV/${file}" "${toString size}"'') files}
            '';

          ovmf-aarch64 = buildOvmf {
            crossPkgs = pkgs.pkgsCross.aarch64-multiplatform;
            dscPath = "ArmVirtPkg/ArmVirtQemu.dsc";
            pname = "OVMF-aarch64";
          };

          ovmf-riscv64-pflash = padFirmware {
            name = "OVMF-riscv64-pflash";
            src = ovmf-riscv64;
            size = 33554432;
            files = [
              "RISCV_VIRT_CODE.fd"
              "RISCV_VIRT_VARS.fd"
            ];
          };

          ovmf-aarch64-pflash = padFirmware {
            name = "OVMF-aarch64-pflash";
            src = ovmf-aarch64;
            size = 67108864;
            files = [
              "QEMU_EFI.fd"
              "QEMU_VARS.fd"
            ];
          };

          qemu = pkgs.qemu.overrideAttrs (_finalAttrs: _prevAttrs: {
            version = "10.2.2";
            src = pkgs.fetchurl {
              url = "https://download.qemu.org/qemu-10.2.2.tar.xz";
              hash = "sha256-eEspb/KcFBeqcjI6vLLS6pq5dxck9Xfc14XDsE8h4XY=";
            };
            configureFlags = (_prevAttrs.configureFlags or [ ]) ++ [
              "--enable-vhost-user"
              "--disable-vhost-net"
            ];
            patches = (_prevAttrs.patches or [ ]) ++ [
              ./nix/patches/qemu-10-cocoa-retina-toggle.patch
            ];
          });

        in
        {
          default = pkgs.mkShell {
            packages = [
              # Rust
              scarletRustToolchain
              pkgs.cargo-make

              # QEMU (system emulation for riscv64 and aarch64)
              qemu

              # Cross-compilation toolchains
              pkgs.pkgsCross.riscv64.buildPackages.gcc
              pkgs.pkgsCross.aarch64-multiplatform.buildPackages.gcc
              pkgs.llvmPackages.llvm

              # Build tools
              pkgs.gcc
              pkgs.gnumake
              pkgs.autoconf
              pkgs.automake
              pkgs.libtool
              pkgs.cmake
              pkgs.ninja
              pkgs.pkg-config

              # Filesystem / image tools
              pkgs.mtools
              pkgs.dosfstools
              pkgs.e2fsprogs
              pkgs.cpio

              # Device tree
              pkgs.dtc

              # Debug
              pkgs.gdb

              # Other dependencies
              pkgs.git
              pkgs.curl
              pkgs.bc
              pkgs.sleuthkit
              pkgs.python3
              pkgs.rsync
            ];

            # EFI firmware paths (consumed by run/test scripts via env vars)
            SCARLET_EFI_CODE_RV64 = "${ovmf-riscv64-pflash}/FV/RISCV_VIRT_CODE.fd";
            SCARLET_EFI_VARS_RV64 = "${ovmf-riscv64-pflash}/FV/RISCV_VIRT_VARS.fd";

            # AArch64 needs two firmware tracks today:
            # - HVF + GICv3 works with QEMU's bundled ArmVirt firmware.
            # - TCG + EL2/VHE currently needs the self-built edk2-stable202502
            #   firmware until the Limine/EDK2 handoff issue is resolved.
            SCARLET_EFI_CODE_ARM64_HVF = "${qemu}/share/qemu/edk2-aarch64-code.fd";
            SCARLET_EFI_VARS_ARM64_HVF = "${qemu}/share/qemu/edk2-arm-vars.fd";
            SCARLET_EFI_CODE_ARM64_EL2 = "${ovmf-aarch64-pflash}/FV/QEMU_EFI.fd";
            SCARLET_EFI_VARS_ARM64_EL2 = "${ovmf-aarch64-pflash}/FV/QEMU_VARS.fd";
            SCARLET_EFI_CODE_ARM64 = "${ovmf-aarch64-pflash}/FV/QEMU_EFI.fd";
            SCARLET_EFI_VARS_ARM64 = "${ovmf-aarch64-pflash}/FV/QEMU_VARS.fd";

            CARGO_NET_GIT_FETCH_WITH_CLI = "true";
            SCARLET_RUST_HOST_TRIPLE = rustHostTriple;
            SCARLET_RUST_TARGET_TRIPLES = "riscv64gc-unknown-scarlet aarch64-unknown-scarlet";
            SCARLET_RUST_TOOLCHAIN = "${scarletRustToolchain}";
          };
        }
      );
    };
}
