#!/bin/sh
# Run in Scarlet's linux-aarch64 view after installing the Linux guest assets.
exec /usr/bin/crosvm --no-syslog run \
  --disable-sandbox --no-pmu --no-rng --no-usb --cpus 1 --mem 512 \
  --serial type=stdout,hardware=serial,num=1,console=true,stdin=true \
  --initrd /usr/share/crosvm/linux/initramfs.cpio \
  --params "earlycon=uart8250,mmio,0x3f8 console=ttyS0 rdinit=/init nokaslr" \
  --block path=/usr/share/crosvm/linux/data.ext2,ro=false,lock=false \
  /usr/share/crosvm/linux/Image
