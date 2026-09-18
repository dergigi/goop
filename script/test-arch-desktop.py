#!/usr/bin/env python3
"""Check the installed Arch launcher's identity and GTK icon lookup under X11."""
from pathlib import Path
import gi

gi.require_version('Gtk', '3.0')
from gi.repository import Gio, Gtk, GdkPixbuf

app_id = 'com.dergigi.goop'
app = Gio.DesktopAppInfo.new(app_id + '.desktop')
assert app is not None and app.should_show(), 'Goop is missing from application discovery'
assert app.get_executable() == 'goop'
assert app.get_startup_wm_class() == app_id
assert app.get_icon().to_string() == app_id
assert not app.get_boolean('Terminal')
assert 'x-scheme-handler/nostr' in app.get_supported_types()
theme = Gtk.IconTheme.get_default()
for size in (16, 24, 32, 48, 64, 128, 256, 512):
    expected = Path(f'/usr/share/icons/hicolor/{size}x{size}/apps/{app_id}.png')
    assert expected.is_file(), expected
    info = theme.lookup_icon(app_id, size, Gtk.IconLookupFlags.FORCE_SIZE)
    assert info is not None, f'Icon lookup failed at {size}px'
    pixbuf = GdkPixbuf.Pixbuf.new_from_file(str(expected))
    assert (pixbuf.get_width(), pixbuf.get_height()) == (size, size)
    assert info.load_icon() is not None
print('Launcher discovery, application identity and icon lookup passed.')
