#!/usr/bin/env bash
# Hardware/environment diagnosis for the Canon LBP2900. Read-only: never unloads
# kernel modules, never changes udev/permissions/VM settings, never sends data.
#
#   scripts/check-hardware.sh [--json FILE]
#
# Exit: 0 printer present and usable (warnings allowed)
#       1 printer present but blocked (permission / endpoints)
#      10 no Canon LBP printer found (hardware tests are SKIPPED)
#       5 environment not suitable (not Linux)
set -uo pipefail

JSON=""
[ "${1:-}" = "--json" ] && JSON=${2:-}

# Report goes to fd 3; anything else a command prints on stdout is caught in STRAY
# and reported as a WARN instead of polluting the output.
STRAY=$(mktemp)
exec 3>&1 1>"$STRAY"

declare -a LINES=()
declare -a JITEMS=()
say() { # say LEVEL CHECK DETAIL
    LINES+=("[$1] $2${3:+: $3}")
    local d=${3//\\/\\\\}; d=${d//\"/\\\"}
    JITEMS+=("{\"level\":\"$1\",\"check\":\"$2\",\"detail\":\"$d\"}")
}
finish() {
    if [ -s "$STRAY" ]; then
        say WARN "Unexpected output" "$(head -c 200 "$STRAY" | tr '\n' ' ')"
    fi
    rm -f "$STRAY"
    exec 1>&3
    printf '%s\n' "${LINES[@]}"
    echo "HARDWARE_AVAILABLE = $AVAILABLE"
    if [ -n "$JSON" ]; then
        mkdir -p "$(dirname "$JSON")"
        local IFS=,
        printf '{"hardware_available":"%s","environment":"%s","printer":"%s","findings":[%s]}\n' \
            "$AVAILABLE" "$ENVIRONMENT" "${PRINTER_ID:-}" "${JITEMS[*]}" >"$JSON"
    fi
    exit "$1"
}

AVAILABLE=NO
ENVIRONMENT=other
PRINTER_ID=""

# 1-2. OS / distribution
if [ "$(uname -s)" != Linux ]; then
    say FAIL "Linux detected" "$(uname -s)"
    finish 5
fi
KERNEL=$(uname -r)
DISTRO=$(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-unknown}")
VENDOR=$(cat /sys/class/dmi/id/sys_vendor 2>/dev/null || true)
if [ -f /.dockerenv ] || grep -qE 'docker|containerd|kubepods' /proc/1/cgroup 2>/dev/null; then ENVIRONMENT=container
else
    # case instead of "echo | grep -q": with pipefail an early-exiting grep fails the pipeline
    case "${KERNEL,,} ${VENDOR,,}" in
        *microsoft*|*wsl*) ENVIRONMENT=wsl ;;
        *vmware*) ENVIRONMENT=vmware ;;
        *innotek*|*virtualbox*) ENVIRONMENT=virtualbox ;;
        *qemu*) ENVIRONMENT=kvm ;;
        *) ENVIRONMENT=native-linux ;;
    esac
fi
say PASS "Linux detected" "$DISTRO, kernel $KERNEL, $(uname -m), environment=$ENVIRONMENT"
[ "$ENVIRONMENT" = wsl ] && say WARN "WSL" "USB only through usbipd; not equivalent to VMware/native"

# 3. libusb
LDCACHE=$(ldconfig -p 2>/dev/null || /sbin/ldconfig -p 2>/dev/null || true)
LIBUSB=$(printf '%s\n' "$LDCACHE" | awk '/libusb-1\.0/ && !f {print $NF; f=1}')
if [ -z "$LIBUSB" ]; then
    for f in /lib/*-linux-gnu/libusb-1.0.so.0 /usr/lib/*-linux-gnu/libusb-1.0.so.0 /usr/lib/libusb-1.0.so.0; do
        [ -e "$f" ] && { LIBUSB=$f; break; }
    done
fi
if [ -n "$LIBUSB" ]; then
    say PASS "libusb detected" "$LIBUSB"
else
    say FAIL "libusb detected" "libusb-1.0 not found (apt install libusb-1.0-0)"
fi

# 4. USB enumeration
if [ ! -d /sys/bus/usb/devices ]; then
    say SKIP "USB enumeration" "/sys/bus/usb not available (no USB in this environment)"
    finish 10
fi
if command -v lsusb >/dev/null; then
    say PASS "USB enumeration" "$(lsusb | wc -l) device(s) via lsusb"
else
    say WARN "USB enumeration" "lsusb missing (apt install usbutils); using sysfs"
fi

# 5. Canon VID/PID
DEV=""
for d in /sys/bus/usb/devices/*; do
    [ -f "$d/idVendor" ] || continue
    [ "$(cat "$d/idVendor")" = 04a9 ] || continue
    prod=$(cat "$d/product" 2>/dev/null || true)
    pid=$(cat "$d/idProduct")
    # our LBP2900 reports product "Canon CAPT USB Device" (not "LBP...")
    case "$pid ${prod^^}" in
        2676\ *|*LBP*|*CAPT*) DEV=$d; break ;;
    esac
done
if [ -z "$DEV" ]; then
    say SKIP "Canon LBP printer" "no Canon LBP device (VID 04a9) on USB — hardware tests SKIPPED"
    finish 10
fi
PID=$(cat "$DEV/idProduct"); BUS=$(cat "$DEV/busnum"); DEVN=$(cat "$DEV/devnum")
MFR=$(cat "$DEV/manufacturer" 2>/dev/null || echo "?"); PROD=$(cat "$DEV/product" 2>/dev/null || echo "?")
PRINTER_ID="04a9:$PID"
if [ "$PID" = 2676 ]; then
    say PASS "Canon LBP2900 detected" "04a9:$PID $MFR $PROD (bus $BUS dev $DEVN)"
else
    say WARN "Canon LBP printer" "04a9:$PID $PROD — unknown revision, not verified"
fi
AVAILABLE=YES
BLOCKED=0

# 6-7. interface / endpoints
IN=""; OUT=""
for intf in "$DEV"/*:*; do
    [ -f "$intf/bInterfaceNumber" ] || continue
    num=$(cat "$intf/bInterfaceNumber"); cls=$(cat "$intf/bInterfaceClass")
    say PASS "Interface" "number $num class $cls/$(cat "$intf/bInterfaceSubClass")/$(cat "$intf/bInterfaceProtocol")"
    for ep in "$intf"/ep_*; do
        [ -f "$ep/type" ] || continue
        [ "$(cat "$ep/type")" = Bulk ] || continue
        addr=$(cat "$ep/bEndpointAddress"); mps=$((16#$(cat "$ep/wMaxPacketSize")))
        if [ "$(cat "$ep/direction")" = in ]; then IN="0x$addr (max $mps B)"; else OUT="0x$addr (max $mps B)"; fi
    done
    # 9. kernel driver claim
    if [ -L "$intf/driver" ]; then
        say WARN "Kernel driver" "interface $num claimed by $(basename "$(readlink "$intf/driver")") — not changed automatically (manual: sudo modprobe -r usblp)"
    else
        say PASS "Kernel driver" "interface $num not claimed"
    fi
done
if [ -n "$IN" ] && [ -n "$OUT" ]; then
    say PASS "Bulk endpoints" "IN $IN, OUT $OUT"
else
    say FAIL "Bulk endpoints" "bulk IN/OUT pair not found"; BLOCKED=1
fi

# 8. permission
NODE=$(printf '/dev/bus/usb/%03d/%03d' "$BUS" "$DEVN")
if [ -w "$NODE" ]; then
    say PASS "USB permission" "$NODE writable by $(id -un)"
else
    say FAIL "USB permission" "$NODE not writable by $(id -un) — install the udev rule (README 'Linux permissions')"; BLOCKED=1
fi

finish $BLOCKED
