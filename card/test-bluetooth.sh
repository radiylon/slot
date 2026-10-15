#!/bin/sh
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SCRIPT="$HERE/System/bluetooth.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

MAC=AA:BB:CC:DD:EE:01
OTHER=AA:BB:CC:DD:EE:02
FAKE="$TMP/fake"
mkdir -p "$FAKE" "$TMP/run"
: > "$FAKE/paired"
: > "$FAKE/connected"
echo off > "$FAKE/power"
echo 30 > "$FAKE/volume"
printf '%s Test Headphones\n%s 11-22-33-44-55-66\n' "$MAC" "$OTHER" > "$FAKE/nearby"

cat > "$TMP/bluetoothctl" <<EOF
#!/bin/sh
F="$FAKE"
name() { grep "^\$1 " "\$F/nearby" | cut -d' ' -f2-; }
case "\${1:-}" in
	show) echo "Controller 00:00:00:00:00:00"; echo "	Powered: \$(sed 's/on/yes/; s/off/no/' "\$F/power")" ;;
	power) echo "\$2" > "\$F/power" ;;
	devices)
		case "\${2:-}" in
			Paired) list="\$F/paired" ;;
			Connected) list="\$F/connected" ;;
			*) list="\$F/nearby" ;;
		esac
		while read -r mac rest; do
			[ -n "\$mac" ] && echo "Device \$mac \$(name "\$mac")"
		done < "\$list"
		;;
	info) grep -q "^\$2\$" "\$F/paired" && echo "	Paired: yes" || echo "	Paired: no" ;;
	connect) grep -q "^\$2\$" "\$F/paired" && ! grep -q "^\$2\$" "\$F/connected" && echo "\$2" >> "\$F/connected"; true ;;
	disconnect) grep -v "^\$2\$" "\$F/connected" > "\$F/c.tmp" || true; mv "\$F/c.tmp" "\$F/connected" ;;
	remove) grep -v "^\$2\$" "\$F/paired" > "\$F/p.tmp" || true; mv "\$F/p.tmp" "\$F/paired" ;;
	"")
		while read -r line; do
			case "\$line" in
				"scan on") while read -r mac rest; do echo "[NEW] Device \$mac \$rest"; done < "\$F/nearby" ;;
				"pair "*)
					mac="\${line#pair }"
					if [ -f "\$F/refuse" ]; then echo "Failed to pair: org.bluez.Error.AuthenticationFailed"
					else echo "\$mac" >> "\$F/paired"; echo "Pairing successful"; fi
					;;
				quit) exit 0 ;;
			esac
		done
		;;
esac
EOF

cat > "$TMP/dbus-send" <<EOF
#!/bin/sh
F="$FAKE"
case "\$*" in
	*GetManagedObjects*)
		while read -r mac; do
			[ -n "\$mac" ] && echo "   object path \"/org/bluez/hci0/dev_\$(echo "\$mac" | tr ':' '_')/sep1/fd0\""
		done < "\$F/connected"
		;;
	*Properties.Get*) echo "   variant       uint16 \$(cat "\$F/volume")" ;;
	*Properties.Set*) for a in "\$@"; do case "\$a" in variant:uint16:*) echo "\${a#variant:uint16:}" > "\$F/volume" ;; esac; done ;;
esac
EOF

cat > "$TMP/bluealsa-aplay" <<EOF
#!/bin/sh
[ -f "$FAKE/silent" ] && exit 0
while read -r mac; do
	[ -n "\$mac" ] && echo "bluealsa:DEV=\$mac,PROFILE=a2dp,SRV=org.bluealsa"
done < "$FAKE/connected"
EOF

printf '#!/bin/sh\necho "hci0: Type: Primary"; echo "	UP RUNNING"\n' > "$TMP/hciconfig"
printf '#!/bin/sh\nexit 0\n' > "$TMP/true"
chmod +x "$TMP/bluetoothctl" "$TMP/dbus-send" "$TMP/bluealsa-aplay" "$TMP/hciconfig" "$TMP/true"

bt() {
	AGS_RUN="$TMP/run" AGS_BT_LOG="$TMP/log" AGS_BLUETOOTHCTL="$TMP/bluetoothctl" \
		AGS_DBUS_SEND="$TMP/dbus-send" AGS_BLUEALSA_LIST="$TMP/bluealsa-aplay" \
		AGS_HCICONFIG="$TMP/hciconfig" AGS_RFKILL="$TMP/true" AGS_PIDOF="$TMP/true" \
		AGS_BT_WAIT_STEP=0.01 sh "$SCRIPT" "$@"
}

ROUTE="$TMP/run/slot-bt-route"

expect() {
	if [ "$1" != "$2" ]; then
		echo "$3: expected [$2], got [$1]" >&2
		exit 1
	fi
}

expect "$(bt up)" "power on" "up turns the radio on"
expect "$(cat "$FAKE/power")" "on" "the radio is powered"

expect "$(bt scan 0.05)" "found $MAC Test Headphones" "a scan lists named headphones and hides bare addresses"

expect "$(bt pair "$MAC")" "connected $MAC" "pairing connects the headphones"
expect "$(cat "$ROUTE")" "bluealsa:DEV=$MAC,PROFILE=a2dp" "a connection routes slot's sound to them"

expect "$(bt status)" "$(printf 'power on\ndevice %s 1 Test Headphones\nvolume 30' "$MAC")" "status reports the pair, its link and its volume"

expect "$(bt volume 50)" "volume 50" "the headphones' own volume is set"
expect "$(cat "$FAKE/volume")" "50" "and stored where BlueZ keeps it"

grep -v "^$MAC\$" "$FAKE/connected" > "$FAKE/c.tmp" || true
mv "$FAKE/c.tmp" "$FAKE/connected"
[ -f "$ROUTE" ] || { echo "a dropped link cleared the route, so the speaker would take over" >&2; exit 1; }

expect "$(bt connect "$MAC")" "connected $MAC" "paired headphones reconnect"

expect "$(bt speaker)" "speaker" "speaker hands the sound back"
[ ! -f "$ROUTE" ] || { echo "speaker left the route in place" >&2; exit 1; }
expect "$(cat "$FAKE/connected")" "" "and drops the headphones"

bt connect "$MAC" > /dev/null
expect "$(bt forget "$MAC")" "forgot $MAC" "forget unpairs"
[ ! -f "$ROUTE" ] || { echo "forgetting the routed pair left its route" >&2; exit 1; }
expect "$(bt status)" "power on" "a forgotten pair is gone from status"

touch "$FAKE/refuse"
if out="$(bt pair "$OTHER")"; then
	echo "a refused pairing reported success: $out" >&2
	exit 1
fi
expect "$out" "error Could not pair" "a refused pairing says why"
rm -f "$FAKE/refuse"

echo "$MAC" >> "$FAKE/paired"
touch "$FAKE/silent"
if out="$(bt connect "$MAC")"; then
	echo "a link without an audio endpoint reported success: $out" >&2
	exit 1
fi
[ ! -f "$ROUTE" ] || { echo "a failed connection routed the sound" >&2; exit 1; }
rm -f "$FAKE/silent"

expect "$(bt down)" "power off" "down turns the radio off"
expect "$(cat "$FAKE/connected")" "" "and drops every link"

if bt connect not-a-mac > /dev/null; then
	echo "a malformed address was accepted" >&2
	exit 1
fi

echo "bluetooth: ok"
