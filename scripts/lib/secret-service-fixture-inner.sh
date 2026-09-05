#!/bin/sh
# Runs inside a private session bus (started by secret-service-fixture.sh's
# `dbus-run-session`) with an unlocked gnome-keyring on it as the one
# Secret Service provider, proves the round trip works, then execs "$@" --
# so the real test binary runs against a real provider rather than a mock.
#
# Never invoked directly: dbus-run-session is what gives this process the
# private bus in the first place.

set -e

# A fixture value, not a secret: this keyring holds nothing but what the
# tests plant, and it is destroyed with the session bus dbus-run-session
# tears down when "$@" exits.
FIXTURE_PASSWORD="glasshouse-ci-fixture-not-a-secret"

gnome_keyring_env="$(mktemp)"
printf '%s' "$FIXTURE_PASSWORD" | gnome-keyring-daemon --unlock --daemonize --components=secrets > "$gnome_keyring_env"
eval "$(grep -E '^[A-Z_]+=' "$gnome_keyring_env" | sed 's/^/export /')"
rm -f "$gnome_keyring_env"

# org.freedesktop.secrets is claimed by the daemon above asynchronously, and
# under load can lose a race against gnome-keyring's own D-Bus
# service-activation file: `busctl --user list` shows an ACTIVATABLE name
# before anything owns it, and a client that talks to it at that point
# (observed: `secret-tool`, under a loaded build host) auto-starts a SECOND,
# unrelated `gnome-keyring-daemon --start --foreground` that races the first
# one's still-being-written keyring file and reads it back as "invalid or
# unrecognized format". GetNameOwner is a passive query -- it never triggers
# activation itself -- and only succeeds once a real owner exists, so
# polling it (rather than `list`) waits for our own daemon specifically.
tries=0
until busctl --user call org.freedesktop.DBus / org.freedesktop.DBus GetNameOwner s org.freedesktop.secrets >/dev/null 2>&1; do
    tries=$((tries + 1))
    if [ "$tries" -ge 50 ]; then
        echo "SECRET SERVICE FIXTURE: org.freedesktop.secrets never got a real owner after 5s" >&2
        exit 1
    fi
    sleep 0.1
done

# One line of proof that a provider is on the bus and unlocked, printed into
# the log -- a round trip through the same CLI a user would reach for
# (`secret-tool`), not just a presence check.
proof_value="glasshouse-ci-fixture-proof-$$"
printf '%s' "$proof_value" | secret-tool store --label=glasshouse-ci-fixture-proof service glasshouse-ci-fixture account proof
looked_up="$(secret-tool lookup service glasshouse-ci-fixture account proof)"
secret-tool clear service glasshouse-ci-fixture account proof
if [ "$looked_up" != "$proof_value" ]; then
    echo "SECRET SERVICE FIXTURE: round trip did not return what was stored" >&2
    exit 1
fi
echo "SECRET SERVICE FIXTURE: org.freedesktop.secrets reachable and unlocked (secret-tool round trip ok)"

exec "$@"
