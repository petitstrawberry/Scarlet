#!/system/bin/sh
# Executed from the optional read-only payload disk after Android boot completes.
exec >/dev/hvc1 2>&1
fail() { echo "SCARLET_ANDROID_APK_FAIL: $*"; exit 1; }
payload=/data/local/tmp/scarlet-apk
package=dev.scarlet.compat
run="scarlet-$(cat /proc/sys/kernel/random/uuid)" || fail nonce
[ "$(getprop sys.boot_completed)" = 1 ] || fail boot
echo SCARLET_ANDROID_BOOT_OK
pm install -r "$payload/probe.apk" || fail install
am force-stop "$package" || fail stop
am start -W -n "$package/.MainActivity" --es scarlet_run "$run" || fail launch
seen() { logcat -d -s ScarletCompat:I '*:S' | grep -F "$1 $run" >/dev/null; }
i=0
until seen SCARLET_APK_RESUMED; do
    i=$((i + 1)); [ "$i" -lt 60 ] || fail resume
    sleep 1
done
echo SCARLET_ANDROID_APK_RESUMED
input tap 400 300 || fail input
i=0
until seen SCARLET_APK_TOUCH; do
    i=$((i + 1)); [ "$i" -lt 60 ] || fail touch
    sleep 1
done
echo SCARLET_ANDROID_APK_INPUT_OK
# Validate the capture on the host; an APK marker alone is not frame evidence.
frame=/data/local/tmp/scarlet-apk-frame.png
screencap -p "$frame" || fail screencap
echo SCARLET_APK_FRAME_BEGIN
base64 "$frame" || fail base64
echo SCARLET_APK_FRAME_END
echo SCARLET_ANDROID_APK_OK
