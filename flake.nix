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
    { self, nixpkgs, rust-overlay }:
    let
      supportedSystems = [ "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs supportedSystems (system: f system);
    in
    {
      devShells = forAllSystems (
        system:
        let
          overlays = [ (import rust-overlay) ];
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

          # UEFI firmware for RISC-V QEMU (cross-compiled via edk2)
          ovmf-riscv64 = pkgs.pkgsCross.riscv64.edk2.mkDerivation
            "OvmfPkg/RiscVVirt/RiscVVirtQemu.dsc"
            { };

          # UEFI firmware for AArch64 QEMU (cross-compiled via edk2)
          ovmf-aarch64 = pkgs.pkgsCross.aarch64-multiplatform.edk2.mkDerivation
            "ArmVirtPkg/ArmVirtQemu.dsc"
            { };

        in
        {
          default = pkgs.mkShell {
            packages = [
              # Rust
              rustToolchain
              pkgs.cargo-make

              # QEMU (system emulation for riscv64 and aarch64)
              pkgs.qemu

              # Cross-compilation toolchains
              pkgs.pkgsCross.riscv64.buildPackages.gcc
              pkgs.pkgsCross.aarch64-multiplatform.buildPackages.gcc

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
            # These paths map to the edk2 build output FV directory.
            # NOTE: If the firmware files are not found at these exact paths,
            # check the actual output with: ls ${ovmf-riscv64}/FV/
            SCARLET_EFI_CODE_RV64 = "${ovmf-riscv64}/FV/RISCV_VIRT_CODE.fd";
            SCARLET_EFI_VARS_RV64 = "${ovmf-riscv64}/FV/RISCV_VIRT_VARS.fd";
            SCARLET_EFI_CODE_ARM64 = "${ovmf-aarch64}/FV/QEMU_EFI.fd";
            SCARLET_EFI_VARS_ARM64 = "${ovmf-aarch64}/FV/QEMU_VARS.fd";

            # Ensure cargo uses git CLI for fetching (consistent with Dockerfile)
            CARGO_NET_GIT_FETCH_WITH_CLI = "true";
          };
        }
      );
    };
}
