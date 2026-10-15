#!/bin/sh
# Bluetooth headphones for slot on BaseOS: BlueZ for the radio, BlueALSA for the sound.
#
#   bluetooth.sh up                  radio on, daemons running
#   bluetooth.sh down                headphones dropped, radio off
#   bluetooth.sh status              power, paired headphones, headphone volume
#   bluetooth.sh scan [seconds]      headphones in pairing mode nearby
#   bluetooth.sh pair <mac>          pair, trust and connect
#   bluetooth.sh connect <mac>       connect paired headphones and route slot's sound to them
#   bluetooth.sh speaker             route slot's sound back to the speaker
#   bluetooth.sh forget <mac>        unpair
#   bluetooth.sh volume [0-127]      the headphones' own volume
#
# Every answer is one record a line: "power on", "device <mac> <connected 0|1> <name>",
# "volume <n>", "found <mac> <name>", "connected <mac>", "error <why>".
# The route file holds the ALSA device slot plays to. It stays while the headphones
# drop out, so the speaker never takes over by itself.
set -u

RUN="${AGS_RUN:-/run}"
ROUTE="${AGS_BT_ROUTE:-$RUN/slot-bt-route}"
LOG="${AGS_BT_LOG:-/tmp/slot-bluetooth.log}"
BT="${AGS_BLUETOOTHCTL:-bluetoothctl}"
DBUS_SEND="${AGS_DBUS_SEND:-dbus-send}"
APLAY_LIST="${AGS_BLUEALSA_LIST:-bluealsa-aplay}"
HCICONFIG="${AGS_HCICONFIG:-hciconfig}"
RFKILL="${AGS_RFKILL:-rfkill}"
ATTACH="${AGS_BT_ATTACH:-/mnt/vendor/ctrl/setBluetooth.sh}"
BLUETOOTHD="${AGS_BLUETOOTHD:-/usr/libexec/bluetooth/bluetoothd}"
BLUEALSA="${AGS_BLUEALSA:-bluealsa}"
DBUS_DAEMON="${AGS_DBUS_DAEMON:-dbus-daemon}"
PIDOF="${AGS_PIDOF:-pidof}"
WAIT_STEP="${AGS_BT_WAIT_STEP:-0.25}"
FIFO="${AGS_BT_FIFO:-$RUN/slot-bt.fifo}"
SESSION="${AGS_BT_SESSION:-$RUN/slot-bt.session}"

log() {
	echo "$(date '+%H:%M:%S') bluetooth: $*" >> "$LOG" 2>/dev/null
	return 0
}

nap() {
	sleep "$WAIT_STEP" 2>/dev/null || sleep 1
}

valid_mac() {
	printf '%s\n' "$1" | grep -Eq '^([0-9A-F]{2}:){5}[0-9A-F]{2}$'
}

# A name that is only the address spelled out is not worth showing.
raw_name() {
	printf '%s\n' "$1" | grep -Eqi '^([0-9a-f]{2}[:-]){5}[0-9a-f]{2}$'
}

ESC="$(printf '\033')"
CR="$(printf '\r')"

plain() {
	sed "s/$ESC\[[0-9;]*[A-Za-z]//g; s/$CR//g"
}

ctl() {
	if command -v timeout > /dev/null 2>&1; then
		timeout 10 "$BT" "$@" 2>>"$LOG" | plain
	else
		"$BT" "$@" 2>>"$LOG" | plain
	fi
}

running() {
	$PIDOF "$1" >/dev/null 2>&1
}

daemons() {
	if ! running dbus-daemon; then
		mkdir -p "$RUN/dbus"
		[ -s "$RUN/machine-id" ] || dbus-uuidgen > "$RUN/machine-id" 2>/dev/null
		nohup "$DBUS_DAEMON" --system < /dev/null >> "$LOG" 2>&1
		w=0
		while [ ! -S "$RUN/dbus/system_bus_socket" ] && [ "$w" -lt 20 ]; do
			nap
			w=$((w + 1))
		done
	fi
	if ! running bluetoothd; then
		nohup "$BLUETOOTHD" < /dev/null >> "$LOG" 2>&1 &
	fi
	if ! running "$(basename "$BLUEALSA")"; then
		nohup "$BLUEALSA" -p a2dp-source < /dev/null >> "$LOG" 2>&1 &
	fi
}

powered() {
	ctl show | grep -q 'Powered: yes'
}

up() {
	$RFKILL unblock bluetooth 2>/dev/null
	if ! $HCICONFIG hci0 2>/dev/null | grep -q 'UP RUNNING'; then
		log "attaching the controller"
		nohup sh "$ATTACH" all < /dev/null >> "$LOG" 2>&1
	fi
	daemons
	w=0
	until powered; do
		[ "$w" -ge 40 ] && { echo "error Bluetooth did not start"; return 1; }
		ctl power on > /dev/null
		nap
		w=$((w + 1))
	done
	echo "power on"
}

down() {
	rm -f "$ROUTE"
	for mac in $(connected_macs); do
		ctl disconnect "$mac" > /dev/null
	done
	ctl power off > /dev/null
	echo "power off"
}

paired_lines() {
	ctl devices Paired | grep '^Device '
}

connected_macs() {
	ctl devices Connected | grep '^Device ' | cut -d' ' -f2
}

transport() {
	dev="/org/bluez/hci0/dev_$(printf '%s' "$1" | tr ':' '_')"
	$DBUS_SEND --system --print-reply --dest=org.bluez / \
		org.freedesktop.DBus.ObjectManager.GetManagedObjects 2>/dev/null |
		grep -o "object path \"$dev/sep[0-9]*/fd[0-9]*\"" | head -1 | cut -d'"' -f2
}

get_volume() {
	tp="$(transport "$1")"
	[ -n "$tp" ] || return 1
	$DBUS_SEND --system --print-reply --dest=org.bluez "$tp" \
		org.freedesktop.DBus.Properties.Get \
		string:org.bluez.MediaTransport1 string:Volume 2>/dev/null |
		awk '/uint16/ { print $NF }' | head -1
}

set_volume() {
	tp="$(transport "$1")"
	[ -n "$tp" ] || return 1
	$DBUS_SEND --system --print-reply --dest=org.bluez "$tp" \
		org.freedesktop.DBus.Properties.Set \
		string:org.bluez.MediaTransport1 string:Volume "variant:uint16:$2" > /dev/null 2>&1
}

status() {
	if powered; then echo "power on"; else echo "power off"; fi
	live=" $(connected_macs | tr '\n' ' ') "
	paired_lines | while read -r _ mac name; do
		valid_mac "$mac" || continue
		case "$live" in
			*" $mac "*) on=1 ;;
			*) on=0 ;;
		esac
		echo "device $mac $on $name"
	done
	for mac in $live; do
		v="$(get_volume "$mac")"
		[ -n "$v" ] && echo "volume $v"
		break
	done
}

# One bluetoothctl kept open through discovery and pairing: BlueZ only answers a
# pairing through an agent that is still registered when the headphones ask.
session_open() {
	rm -f "$FIFO" "$SESSION"
	mkfifo "$FIFO" || return 1
	"$BT" < "$FIFO" > "$SESSION" 2>&1 &
	SESSION_PID=$!
	exec 3> "$FIFO"
	say "agent NoInputNoOutput"
	say "default-agent"
	say "pairable on"
	say "menu scan"
	say "transport bredr"
	say "back"
}

say() {
	printf '%s\n' "$1" >&3
}

session_close() {
	say "scan off"
	say "quit"
	exec 3>&-
	w=0
	while kill -0 "$SESSION_PID" 2>/dev/null && [ "$w" -lt 8 ]; do
		nap
		w=$((w + 1))
	done
	kill "$SESSION_PID" 2>/dev/null
	plain < "$SESSION" >> "$LOG"
	rm -f "$FIFO"
}

heard() {
	plain < "$SESSION" | grep -q "$1"
}

scan() {
	secs="${1:-12}"
	session_open || { echo "error no session"; return 1; }
	say "scan on"
	steps=$(awk "BEGIN { print int($secs / $WAIT_STEP) }")
	w=0
	while [ "$w" -lt "$steps" ]; do
		nap
		w=$((w + 1))
	done
	say "scan off"
	session_close
	ctl devices | grep '^Device ' | while read -r _ mac name; do
		valid_mac "$mac" || continue
		[ -n "$name" ] && ! raw_name "$name" || continue
		echo "found $mac $name"
	done
}

pair() {
	mac="$1"
	session_open || { echo "error no session"; return 1; }
	say "scan on"
	w=0
	until heard "Device $mac" || [ "$w" -ge 60 ]; do
		nap
		w=$((w + 1))
	done
	say "scan off"
	say "pair $mac"
	w=0
	until heard "Pairing successful" || heard "Failed to pair" || heard "AlreadyExists" || [ "$w" -ge 80 ]; do
		nap
		w=$((w + 1))
	done
	say "trust $mac"
	nap
	session_close
	if ! ctl info "$mac" | grep -q 'Paired: yes'; then
		echo "error Could not pair"
		return 1
	fi
	connect "$mac"
}

pcm_ready() {
	$APLAY_LIST -L 2>/dev/null | grep -q "DEV=$1"
}

connect() {
	mac="$1"
	for attempt in 1 2; do
		ctl connect "$mac" > /dev/null
		w=0
		until pcm_ready "$mac" || [ "$w" -ge 32 ]; do
			nap
			w=$((w + 1))
		done
		pcm_ready "$mac" && break
		log "connect $mac attempt $attempt found no audio endpoint"
	done
	if ! pcm_ready "$mac"; then
		echo "error Could not connect"
		return 1
	fi
	mkdir -p "$(dirname "$ROUTE")"
	printf 'bluealsa:DEV=%s,PROFILE=a2dp\n' "$mac" > "$ROUTE.$$"
	mv -f "$ROUTE.$$" "$ROUTE"
	echo "connected $mac"
}

speaker() {
	rm -f "$ROUTE"
	for mac in $(connected_macs); do
		ctl disconnect "$mac" > /dev/null
	done
	echo "speaker"
}

forget() {
	mac="$1"
	if grep -q "DEV=$mac," "$ROUTE" 2>/dev/null; then
		rm -f "$ROUTE"
	fi
	ctl disconnect "$mac" > /dev/null
	ctl remove "$mac" > /dev/null
	echo "forgot $mac"
}

volume() {
	mac="$(connected_macs | head -1)"
	[ -n "$mac" ] || { echo "error No headphones connected"; return 1; }
	if [ -n "${1:-}" ]; then
		set_volume "$mac" "$1" || { echo "error No volume control"; return 1; }
	fi
	v="$(get_volume "$mac")"
	[ -n "$v" ] || { echo "error No volume control"; return 1; }
	echo "volume $v"
}

need_mac() {
	valid_mac "${1:-}" || { echo "error not an address: ${1:-}"; exit 2; }
}

verb="${1:-}"
[ "$#" -gt 0 ] && shift
log "$verb $*"
case "$verb" in
	up) up ;;
	down) down ;;
	status) status ;;
	scan) scan "${1:-12}" ;;
	pair) need_mac "${1:-}"; pair "$1" ;;
	connect) need_mac "${1:-}"; connect "$1" ;;
	speaker) speaker ;;
	forget) need_mac "${1:-}"; forget "$1" ;;
	volume) volume "${1:-}" ;;
	*) echo "error unknown verb: $verb"; exit 2 ;;
esac
