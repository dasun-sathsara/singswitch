#!/usr/bin/env python3
import sys
import subprocess
import time
import os
from PIL import Image

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

# Start isolated Xvfb
xvfb = subprocess.Popen(["Xvfb", display, "-screen", "0", "1280x800x24"])
time.sleep(0.4)

app = None
try:
    cmd = ["./target/debug/singswitch-app", "--preview", scene, "--theme", theme]
    app = subprocess.Popen(cmd, env=env)
    time.sleep(2.0)

    raw_path = "/tmp/raw_snap.png"
    subprocess.run(["import", "-window", "root", raw_path], env=env, check=True)

    img = Image.open(raw_path)
    bbox = img.getbbox()
    if bbox:
        # Bounding box of the rendered window on the black background
        cropped = img.crop(bbox)
        cropped.save(out_png)
    else:
        img.save(out_png)
    print(f"Captured {scene} ({theme}) to {out_png}")
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
