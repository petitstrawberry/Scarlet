#!/bin/sh
set -eu
/bin/busybox --install -s /bin
mount -t devtmpfs devtmpfs /dev
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t tmpfs tmpfs /tmp
echo CROSVM_LINUX_INIT
uname -a
sleep 1
(sh -c 'echo fork-ok > /tmp/fork') &
wait
test "$(cat /tmp/fork)" = fork-ok
printf 'guest-filesystem-ok\n' > /tmp/check
test "$(cat /tmp/check)" = guest-filesystem-ok
cat /proc/uptime
if grep -qw scarlet.block_check=1 /proc/cmdline; then
    mount -t ext4 /dev/vda /mnt
    test "$(cat /mnt/input)" = virtio-input
    printf 'virtio-write-ok\n' > /mnt/output
    sync
    umount /mnt
    mount -t ext4 /dev/vda /mnt
    test "$(cat /mnt/output)" = virtio-write-ok
    umount /mnt
    echo SCARLET_CROSVM_LINUX_BLOCK_OK
else
    echo SCARLET_CROSVM_LINUX_OK
fi
exec setsid cttyhack sh
