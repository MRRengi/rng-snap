import sys
from Xlib import display, Xatom
d = display.Display(); r = d.screen().root
r.change_property(d.intern_atom("RESOURCE_MANAGER"), Xatom.STRING, 8, f"Xft.dpi:\t{sys.argv[1]}\n".encode())
d.sync()
