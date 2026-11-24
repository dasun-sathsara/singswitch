#!/usr/bin/env python3
import sys
import subprocess
import tempfile
import time
import os
from PIL import Image, ImageDraw, ImageFilter

if len(sys.argv) < 4:
    print("Usage: snap.py <scene> <theme> <output_png>")
    sys.exit(1)

scene = sys.argv[1]
theme = sys.argv[2]
out_png = sys.argv[3]

# Ports are shared between checkouts: a second capture on the same :NN either
# fails to start Xvfb or, worse, screenshots the other run's window. Allow an
# override and verify the server we started is the one we are about to use.
display = os.environ.get("SINGSWITCH_SNAP_DISPLAY", ":95")
env = os.environ.copy()
env["DISPLAY"] = display
env["VK_DRIVER_FILES"] = "/usr/share/vulkan/icd.d/lvp_icd.json"
env["SINGSWITCH_PREVIEW"] = scene
env["SINGSWITCH_THEME"] = theme
env["GPUI_X11_SCALE_FACTOR"] = "2.0"

# Start isolated Xvfb with high-res virtual display
xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "2560x1600x24", "-nolisten", "tcp"])
time.sleep(0.5)

# The preview's scene match is exact (dashes, not underscores), and an unknown
# name silently falls back to the connections scene: two screenshots of
# different screens then come out identical with no error. Refuse to capture
# an unknown scene instead of shipping the fallback by accident.
KNOWN_SCENES = {
    "empty",
    "connections",
    "profiles",
    "notice",
    "editor-guided",
    "editor-json",
    "logs",
    "settings",
    "nodes",
}
if scene not in KNOWN_SCENES:
    print(
        f"error: unknown scene {scene!r}; known scenes: {', '.join(sorted(KNOWN_SCENES))}",
        file=sys.stderr,
    )
    sys.exit(1)
if xvfb.poll() is not None:
    print(
        f"error: Xvfb exited on {display} (exit {xvfb.returncode}); "
        "another display may already be in use. Set SINGSWITCH_SNAP_DISPLAY=:NN.",
        file=sys.stderr,
    )
    sys.exit(1)

app = None
raw_path = None
try:
    cmd = ["./target/debug/singswitch-app", "--preview", scene, "--theme", theme]
    app = subprocess.Popen(cmd, env=env)
    time.sleep(2.5)

    # A fixed /tmp path means concurrent captures overwrite each other's frame.
    raw_fd, raw_path = tempfile.mkstemp(prefix="singswitch-snap-", suffix=".png")
    os.close(raw_fd)
    subprocess.run(["import", "-window", "root", raw_path], env=env, check=True)

    img = Image.open(raw_path)
    img.load()
    bbox = img.getbbox()
    if bbox:
        cropped = img.crop(bbox)
        
        # Frame the window with rounded corners, padding, and soft shadow
        pad_x = 44
        pad_y = 38
        radius = 24
        
        w, h = cropped.size
        mask = Image.new("L", (w, h), 0)
        mdraw = ImageDraw.Draw(mask)
        mdraw.rounded_rectangle([(0, 0), (w, h)], radius=radius, fill=255)
        
        window_rgba = cropped.convert("RGBA")
        window_rgba.putalpha(mask)
        
        # Canvas with soft drop shadow
        canvas = Image.new("RGBA", (w + pad_x * 2, h + pad_y * 2), (0, 0, 0, 0))
        sdraw = ImageDraw.Draw(canvas)
        shadow_color = (0, 0, 0, 140 if theme == "dark" else 70)
        sdraw.rounded_rectangle(
            [(pad_x, pad_y + 12), (pad_x + w, pad_y + 12 + h)],
            radius=radius,
            fill=shadow_color,
        )
        canvas = canvas.filter(ImageFilter.GaussianBlur(22))
        
        # Composite window onto canvas
        canvas.paste(window_rgba, (pad_x, pad_y), window_rgba)
        canvas.save(out_png)
    else:
        img.save(out_png)
    print(f"Captured {scene} ({theme}) @2x to {out_png}")
finally:
    if app:
        app.terminate()
        try:
            app.wait(timeout=1.0)
        except Exception:
            app.kill()
    xvfb.terminate()
    try:
        xvfb.wait(timeout=1.0)
    except Exception:
        xvfb.kill()
    if raw_path and os.path.exists(raw_path):
        os.unlink(raw_path)
