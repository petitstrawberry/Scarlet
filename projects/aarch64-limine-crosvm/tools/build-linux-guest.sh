#!/bin/sh
# Run inside scarlet-crosvm-builder with an output directory mounted at /export.
set -eu
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y -qq flex bison bc libelf-dev busybox-static xz-utils cpio >/dev/null
curl -fL --retry 3 https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-6.12.tar.xz -o /tmp/linux.tar.xz
echo 'b1a2562be56e42afb3f8489d4c2a7ac472ac23098f1ef1c1e40da601f54625eb  /tmp/linux.tar.xz' | sha256sum -c -
sha256sum /tmp/linux.tar.xz > /export/linux-source.sha256
dpkg-query -W busybox-static gcc > /export/build-packages.txt
tar -xJf /tmp/linux.tar.xz -C /tmp
cd /tmp/linux-6.12
make ARCH=arm64 defconfig
scripts/config --enable SERIAL_8250 --enable SERIAL_8250_CONSOLE --enable SERIAL_OF_PLATFORM --enable PCI_HOST_GENERIC --enable VIRTIO_PCI --enable VIRTIO_BLK --enable VIRTIO_MMIO --enable BLK_DEV_INITRD --enable DEVTMPFS --enable DEVTMPFS_MOUNT --enable EXT4_FS --enable IKCONFIG --enable IKCONFIG_PROC --disable DEBUG_INFO --disable DEBUG_INFO_DWARF_TOOLCHAIN_DEFAULT --disable DEBUG_INFO_BTF
make ARCH=arm64 olddefconfig
make -j6 ARCH=arm64 Image > /export/kernel-build.log 2>&1
cp arch/arm64/boot/Image /export/Image
cp vmlinux /export/vmlinux
cp .config /export/linux.config
cp /bin/busybox /export/busybox
