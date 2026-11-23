#!/usr/bin/env python3
import sys
import subprocess
import time
import os
from PIL import Image, ImageDraw, ImageFilter

if len(sys.argv) < 4:
    print("Usage: snap.py <scene> <theme> <output_png>")
    sys.exit(1)

scene = sys.argv[1]
theme = sys.argv[2]
out_png = sys.argv[3]

display = ":95"
env = os.environ.copy()
env["DISPLAY"] = display
env["VK_DRIVER_FILES"] = "/usr/share/vulkan/icd.d/lvp_icd.json"
env["SINGSWITCH_PREVIEW"] = scene
env["SINGSWITCH_THEME"] = theme
env["GPUI_X11_SCALE_FACTOR"] = "2.0"

# Start isolated Xvfb with high-res virtual display
xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "2560x1600x24"])
time.sleep(0.5)

app = None
try:
    cmd = ["./target/debug/singswitch-app", "--preview", scene, "--theme", theme]
    app = subprocess.Popen(cmd, env=env)
    time.sleep(2.5)

    raw_path = "/tmp/raw_snap.png"
    subprocess.run(["import", "-window", "root", raw_path], env=env, check=True)

    img = Image.open(raw_path)
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
