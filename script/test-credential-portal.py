#!/usr/bin/env python3
"""Run the Linux regression against a private bus, never the user's keyring.

Invoke inside dbus-run-session on an ephemeral CI runner with /.flatpak-info.
Only method names and synthetic fixture errors are exchanged; no credentials.
"""
import os
import subprocess
import sys
import threading

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
mode = sys.argv[1]
assert mode in ("missing", "denied")
assert os.path.isfile("/.flatpak-info")


class Portal(dbus.service.Object):
    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="ss", out_signature="v")
    def Get(self, interface, name):
        if mode == "missing":
            raise dbus.exceptions.DBusException(
                "No such interface org.freedesktop.portal.Secret",
                name="org.freedesktop.DBus.Error.InvalidArgs",
            )
        return dbus.UInt32(1)

    @dbus.service.method("org.freedesktop.DBus.Properties", in_signature="s", out_signature="a{sv}")
    def GetAll(self, interface):
        # zbus fills its property cache with GetAll before attempting Get.
        # Missing interfaces must fail identically through both methods.
        return {"version": self.Get(interface, "version")}

    @dbus.service.method("org.freedesktop.portal.Secret", in_signature="ha{sv}", out_signature="o")
    def RetrieveSecret(self, fd, options):
        raise dbus.exceptions.DBusException(
            "goop-test-portal-denied", name="org.freedesktop.portal.Error.NotAllowed"
        )


class Vault(dbus.service.Object):
    @dbus.service.method("org.freedesktop.Secret.Service", in_signature="sv", out_signature="vo")
    def OpenSession(self, algorithm, parameters):
        # This distinguishes actually attempting Secret Service from merely
        # reporting a different portal error. Never start a real system vault.
        raise dbus.exceptions.DBusException(
            "goop-test-host-reached", name="org.freedesktop.DBus.Error.Failed"
        )


portal_name = dbus.service.BusName("org.freedesktop.portal.Desktop", bus)
vault_name = dbus.service.BusName("org.freedesktop.secrets", bus)
portal = Portal(bus, "/org/freedesktop/portal/desktop")
vault = Vault(bus, "/org/freedesktop/secrets")
loop = GLib.MainLoop()
result = []


def run_test():
    env = dict(os.environ, GOOP_CREDENTIAL_PORTAL_TEST=mode)
    try:
        result.append(subprocess.run([
            "cargo", "test", "--locked", "-p", "state", "--lib",
            "credentials::linux::tests::portal_fallback_on_private_bus",
            "--", "--ignored", "--exact", "--nocapture",
        ], env=env, timeout=300).returncode)
    except Exception as error:
        print(error, file=sys.stderr)
        result.append(1)
    finally:
        GLib.idle_add(loop.quit)


threading.Thread(target=run_test).start()
loop.run()
sys.exit(result[0])
