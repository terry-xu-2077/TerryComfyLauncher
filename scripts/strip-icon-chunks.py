"""Strip bad cHRM/iCCP chunks from launcher icon files (kills the libpng warning).

PIL re-save drops ancillary chunks (cHRM, iCCP, gAMA) unless explicitly passed,
which is exactly what we want: keep pixels, drop mismatched color metadata.
"""
from pathlib import Path
from PIL import Image

SRC = Path(r"G:\AIGC\TerryComfyLauncher\frontend\src-tauri")
targets = [SRC / "app-icon.png"] + sorted((SRC / "icons").glob("*.png"))

for p in targets:
    img = Image.open(p)
    img.load()
    img.save(p, format="PNG")
    print("rewrote", p.name, img.mode, img.size)

ico = SRC / "icons" / "icon.ico"
if ico.exists():
    img = Image.open(ico)
    img.save(ico, format="ICO", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
    print("rewrote icon.ico")
