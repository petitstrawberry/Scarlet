{
  description = "Scarlet OS kernel development environment";

  nixConfig = {
    extra-substituters = [ "https://scarlet-rust-toolchain.cachix.org" ];
    extra-trusted-public-keys = [
      "scarlet-rust-toolchain.cachix.org-1:p+coBExi0nNTIvWF/oM9H9/1/GhwFtqGZ2Vs+4pYl6o="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    scarlet-rust-toolchain.url = "github:petitstrawberry/scarlet-rust-nix";
    scarlet-sdk = {
      url = "github:petitstrawberry/scarlet-sdk";
      flake = false;
    };
    macvdmtool-src = {
      url = "github:AsahiLinux/macvdmtool";
      flake = false;
    };
    qemu-cocoa-virgl = {
      url = "git+https://gitlab.com/petitstrawberry/qemu.git?ref=cocoa-virgl&submodules=1";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      scarlet-rust-toolchain,
      scarlet-sdk,
      macvdmtool-src,
      qemu-cocoa-virgl,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      linuxSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f system);
      forLinuxSystems = f: nixpkgs.lib.genAttrs linuxSystems (system: f system);

      mkSystem =
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
          ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };

          rustToolchain = scarlet-rust-toolchain.packages.${system}.scarlet-rust-toolchain;

          keycodemapdb = pkgs.fetchurl {
            url = "https://gitlab.com/qemu-project/keycodemapdb/-/archive/f5772a62ec52591ff6870b7e8ef32482371f22c6/keycodemapdb-f5772a62ec52591ff6870b7e8ef32482371f22c6.tar.gz";
            hash = "sha256-0BS1M4LbsXuBlq0S9Q3n8g0O8bn31UsL5RpsuxQgkZU=";
          };

          berkeley-softfloat-3 = pkgs.fetchurl {
            url = "https://gitlab.com/qemu-project/berkeley-softfloat-3/-/archive/b64af41c3276f97f0e181920400ee056b9c88037/berkeley-softfloat-3-b64af41c3276f97f0e181920400ee056b9c88037.tar.gz";
            hash = "sha256-+q6ImBTqaikvfKA9mzbmx+lbqypkd3gEiDzIIrjUh1c=";
          };

          berkeley-testfloat-3 = pkgs.fetchurl {
            url = "https://gitlab.com/qemu-project/berkeley-testfloat-3/-/archive/e7af9751d9f9fd3b47911f51a5cfd08af256a9ab/berkeley-testfloat-3-e7af9751d9f9fd3b47911f51a5cfd08af256a9ab.tar.gz";
            hash = "sha256-56CdUdx+lsuEIskZyF/Dgz1PeIVnY4yRYu9c19tZsd8=";
          };

          virglrenderer = pkgs.virglrenderer.overrideAttrs (_finalAttrs: prevAttrs: {
            patches = (prevAttrs.patches or [ ]) ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              ./nix/patches/virglrenderer-macos-kqueue-sync.patch
            ];
          });

          cargo-scarlet = pkgs.rustPlatform.buildRustPackage {
            pname = "cargo-scarlet";
            version = "0.1.0";
            src = scarlet-sdk;
            buildAndTestSubdir = "cargo-scarlet";
            cargoLock.lockFile = "${scarlet-sdk}/Cargo.lock";
          };

          cargo-scarlet-plugin-limine = pkgs.rustPlatform.buildRustPackage {
            pname = "cargo-scarlet-plugin-limine";
            version = "0.1.0";
            src = scarlet-sdk;
            buildAndTestSubdir = "cargo-scarlet-plugin-limine";
            cargoLock.lockFile = "${scarlet-sdk}/Cargo.lock";
          };

          macvdmtool =
            if pkgs.stdenv.isDarwin then
              pkgs.stdenv.mkDerivation {
                pname = "macvdmtool";
                version = "0-unstable-2024";
                src = macvdmtool-src;
                installPhase = ''
                  install -Dm755 macvdmtool $out/bin/macvdmtool
                '';
              }
            else
              null;

          rustHostTriple =
            {
              x86_64-linux = "x86_64-unknown-linux-gnu";
              aarch64-linux = "aarch64-unknown-linux-gnu";
              x86_64-darwin = "x86_64-apple-darwin";
              aarch64-darwin = "aarch64-apple-darwin";
            }
            .${system};

          rustBootstrapConfig = pkgs.writeText "scarlet-rust-bootstrap.toml" ''
            change-id = "ignore"

            [build]
            patch-binaries-for-nix = true

            [llvm]
            download-ci-llvm = false
          '';

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
            version = "11.0.91-cocoa-virgl";
            src = qemu-cocoa-virgl;
            configureFlags = (_prevAttrs.configureFlags or [ ]) ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              "--enable-cocoa"
              "--enable-hvf"
            ] ++ [
              "--enable-opengl"
              "--enable-virglrenderer"
              "--enable-vhost-user"
              "--disable-vhost-net"
              "--target-list=aarch64-softmmu,riscv64-softmmu"
            ];
            buildInputs = (_prevAttrs.buildInputs or [ ]) ++ [
              pkgs.libepoxy
              virglrenderer
            ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
              # QEMU 11 AArch64 HVF EL2/SME2 needs Apple SDK 15+; the
              # nixpkgs Darwin stdenv may otherwise default to SDK 14.4.
              pkgs.apple-sdk_15
            ];
            nativeBuildInputs = (_prevAttrs.nativeBuildInputs or [ ]) ++ [ pkgs.git ];
            patches = (_prevAttrs.patches or [ ]) ++ [
              ./nix/patches/qemu-10-cocoa-retina-toggle.patch
            ];
            postPatch = (_prevAttrs.postPatch or "") + ''
              ${pkgs.gnutar}/bin/tar -xf ${keycodemapdb} -C subprojects
              rm -rf subprojects/keycodemapdb
              mv subprojects/keycodemapdb-f5772a62ec52591ff6870b7e8ef32482371f22c6 subprojects/keycodemapdb
              rm -f subprojects/keycodemapdb.wrap
              ${pkgs.gnutar}/bin/tar -xf ${berkeley-softfloat-3} -C subprojects
              rm -rf subprojects/berkeley-softfloat-3
              mv subprojects/berkeley-softfloat-3-b64af41c3276f97f0e181920400ee056b9c88037 subprojects/berkeley-softfloat-3
              cp -R subprojects/packagefiles/berkeley-softfloat-3/. subprojects/berkeley-softfloat-3/
              rm -f subprojects/berkeley-softfloat-3.wrap
              ${pkgs.gnutar}/bin/tar -xf ${berkeley-testfloat-3} -C subprojects
              rm -rf subprojects/berkeley-testfloat-3
              mv subprojects/berkeley-testfloat-3-e7af9751d9f9fd3b47911f51a5cfd08af256a9ab subprojects/berkeley-testfloat-3
              cp -R subprojects/packagefiles/berkeley-testfloat-3/. subprojects/berkeley-testfloat-3/
              rm -f subprojects/berkeley-testfloat-3.wrap
            '';
            # nixpkgs creates qemu-kvm for the host architecture, but this
            # build intentionally omits x86_64-softmmu. Remove the resulting
            # dangling compatibility link on x86_64 Linux.
            postInstall = (_prevAttrs.postInstall or "") + ''
              if [ ! -e "$out/bin/qemu-system-${pkgs.stdenv.hostPlatform.qemuArch}" ]; then
                rm -f "$out/bin/qemu-kvm"
              fi
            '';
          });

          devPackages = [
            # Rust
            rustToolchain
            pkgs.cargo-make

            # Scarlet SDK
            cargo-scarlet
            cargo-scarlet-plugin-limine

            # QEMU (system emulation for riscv64 and aarch64)
            qemu

            # Cross-compilation toolchains
            pkgs.pkgsCross.riscv64.buildPackages.gcc
            pkgs.pkgsCross.aarch64-multiplatform.buildPackages.gcc
            pkgs.llvmPackages.llvm

            # Build tools
            pkgs.bashInteractive
            pkgs.gcc
            pkgs.clang
            pkgs.lld
            pkgs.gnumake
            pkgs.autoconf
            pkgs.automake
            pkgs.libtool
            pkgs.cmake
            pkgs.ninja
            pkgs.pkg-config
            pkgs.bison
            pkgs.flex
            pkgs.meson
            pkgs.texinfo
            pkgs.which
            pkgs.file
            pkgs.patch
            pkgs.perl
            pkgs.gnused
            pkgs.gnugrep
            pkgs.gawk
            pkgs.findutils
            pkgs.diffutils
            pkgs.gnutar
            pkgs.gzip
            pkgs.bzip2
            pkgs.xz
            pkgs.unzip
            pkgs.fontconfig
            pkgs.dejavu_fonts

            # Filesystem / image tools
            pkgs.coreutils
            pkgs.mtools
            pkgs.dosfstools
            pkgs.e2fsprogs
            pkgs.cpio

            # Device tree
            pkgs.dtc

            # Debug
            pkgs.gdb
            pkgs.picocom

            # Other dependencies
            pkgs.git
            pkgs.curl
            pkgs.wget
            pkgs.bc
            pkgs.sleuthkit
            pkgs.python3
            pkgs.python313Packages.pyserial
            pkgs.python313Packages.construct
            pkgs.rsync
            pkgs.ncurses
            pkgs.openssl
            pkgs.libffi
            pkgs.zlib
            pkgs.vim
          ] ++ pkgs.lib.optional pkgs.stdenv.isDarwin macvdmtool;

          devEnv = {
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
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
              pkgs.stdenv.cc.cc.lib
              pkgs.xz
              pkgs.zlib
            ];
            NIX_PATH = "nixpkgs=${pkgs.path}";
            RUST_BOOTSTRAP_CONFIG = "${rustBootstrapConfig}";
            FONTCONFIG_FILE = "${pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; }}";
            SCARLET_RUST_HOST_TRIPLE = rustHostTriple;
            SCARLET_RUST_TARGET_TRIPLES = "riscv64gc-unknown-scarlet aarch64-unknown-scarlet";
            SCARLET_CACHED_RUST_TOOLCHAIN = "${rustToolchain}";
            SCARLET_RUST_TOOLCHAIN = "${rustToolchain}";
          };

          dockerEntrypoint = pkgs.writeShellScriptBin "scarlet-dev-entrypoint" ''
            set -e

            if [ "$#" -eq 0 ]; then
              set -- bash
            fi

            export SCARLET_REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
            export SCARLET_RUST_ACTIVE_BIN="${rustToolchain}/bin"

            if [ -f "$SCARLET_REPO_ROOT/scripts/scarlet-rust-dev.sh" ]; then
              source "$SCARLET_REPO_ROOT/scripts/scarlet-rust-dev.sh"
            fi

            exec "$@"
          '';
        in
        rec {
          devShell = pkgs.mkShell (
            devEnv
            // {
              packages = devPackages;
              # The Nix clang wrapper injects -fzero-call-used-regs=used-gpr
              # when zerocallusedregs is enabled. That flag is not supported for
              # riscv64-unknown-scarlet, and breaks C dependencies such as ring.
              hardeningDisable = [ "zerocallusedregs" ];
              shellHook = ''
              export PATH="${rustToolchain}/bin:$PATH"
              export SCARLET_REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              export SCARLET_RUST_ACTIVE_BIN="${rustToolchain}/bin"
              source "$SCARLET_REPO_ROOT/scripts/scarlet-rust-dev.sh"
            '';
            }
          );

          dockerImage = pkgs.dockerTools.buildLayeredImage {
            name = "scarlet-dev";
            tag = "latest";
            maxLayers = 120;
            contents = [
              cargo-scarlet
              cargo-scarlet-plugin-limine
            ] ++ devPackages ++ [
              dockerEntrypoint
              pkgs.coreutils
              pkgs.cacert
            ];
            config = {
              WorkingDir = "/workspaces/Scarlet";
              Entrypoint = [ "/bin/scarlet-dev-entrypoint" ];
              Cmd = [ "bash" ];
              Env = [
                "PATH=/bin"
                "CARGO_NET_GIT_FETCH_WITH_CLI=${devEnv.CARGO_NET_GIT_FETCH_WITH_CLI}"
                "LD_LIBRARY_PATH=${devEnv.LD_LIBRARY_PATH}"
                "NIX_PATH=${devEnv.NIX_PATH}"
                "RUST_BOOTSTRAP_CONFIG=${devEnv.RUST_BOOTSTRAP_CONFIG}"
                "FONTCONFIG_FILE=${devEnv.FONTCONFIG_FILE}"
                "SCARLET_RUST_HOST_TRIPLE=${devEnv.SCARLET_RUST_HOST_TRIPLE}"
                "SCARLET_RUST_TARGET_TRIPLES=${devEnv.SCARLET_RUST_TARGET_TRIPLES}"
                "SCARLET_CACHED_RUST_TOOLCHAIN=${devEnv.SCARLET_CACHED_RUST_TOOLCHAIN}"
                "SCARLET_RUST_TOOLCHAIN=${devEnv.SCARLET_RUST_TOOLCHAIN}"
                "SCARLET_EFI_CODE_RV64=${devEnv.SCARLET_EFI_CODE_RV64}"
                "SCARLET_EFI_VARS_RV64=${devEnv.SCARLET_EFI_VARS_RV64}"
                "SCARLET_EFI_CODE_ARM64_HVF=${devEnv.SCARLET_EFI_CODE_ARM64_HVF}"
                "SCARLET_EFI_VARS_ARM64_HVF=${devEnv.SCARLET_EFI_VARS_ARM64_HVF}"
                "SCARLET_EFI_CODE_ARM64_EL2=${devEnv.SCARLET_EFI_CODE_ARM64_EL2}"
                "SCARLET_EFI_VARS_ARM64_EL2=${devEnv.SCARLET_EFI_VARS_ARM64_EL2}"
                "SCARLET_EFI_CODE_ARM64=${devEnv.SCARLET_EFI_CODE_ARM64}"
                "SCARLET_EFI_VARS_ARM64=${devEnv.SCARLET_EFI_VARS_ARM64}"
              ];
            };
          };

          packages = pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
            scarlet-dev-image = dockerImage;
            default = dockerImage;
          };
        };
    in
    {
      devShells = forAllSystems (system: {
        default = (mkSystem system).devShell;
      });
      packages = forLinuxSystems (system: (mkSystem system).packages);
    };
}
