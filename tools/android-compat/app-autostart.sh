#!/system/bin/sh
# Manager-requested app launch after stock Android has unlocked user 0.
# Observe the framework state; do not advance its boot/unlock lifecycle.
component="$1"
package="${component%%/*}"
attempt=0
previous_state=
while [ "$attempt" -lt 120 ]; do
    state=$(/system/bin/cmd activity get-started-user-state 0) || exit 1
    if [ "$state" != "$previous_state" ]; then
        echo "Android app launch waiting for user 0: $state"
        previous_state="$state"
    fi
    if [ "$state" = RUNNING_UNLOCKED ]; then
        /system/bin/cmd package install-existing --user 0 "$package" || exit 1
        exec /system/bin/cmd activity start -W --user 0 -f 0x30000000 -n "$component"
    fi
    attempt=$((attempt + 1))
    /system/bin/toybox sleep 1 || exit 1
done
echo "Android user 0 did not unlock within the app launch wait" >&2
exit 1
