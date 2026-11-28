# Streams the whole video at 60 fps, one cropped frame at a time (about 1.4 MB each).
# Usage: average.py VIDEO OUT. It writes mean.png and std.npy to OUT.
import subprocess, sys, numpy as np
from PIL import Image
src, out = sys.argv[1], sys.argv[2]
X0, Y0, X1, Y1 = 120, 760, 1760, 1040
p = subprocess.Popen(["ffmpeg","-hide_banner","-loglevel","error","-i",src,"-vf",f"crop={X1-X0}:{Y1-Y0}:{X0}:{Y0}",
                      "-f","rawvideo","-pix_fmt","rgb24","-"], stdout=subprocess.PIPE)
cs = (Y1-Y0) * (X1-X0) * 3
s = np.zeros((Y1-Y0, X1-X0, 3)); s2 = np.zeros_like(s); n = 0
while True:
    buf = p.stdout.read(cs)
    if len(buf) < cs: break
    f = np.frombuffer(buf, np.uint8).reshape(Y1-Y0, X1-X0, 3).astype(float)
    s += f; s2 += f * f; n += 1
p.wait()
mean = s / n
std = np.sqrt(np.maximum(s2 / n - mean ** 2, 0)).mean(axis=2)
print("frames", n)
Image.fromarray(np.clip(mean + .5, 0, 255).astype(np.uint8)).save(f"{out}/mean.png")
np.save(f"{out}/std.npy", std.astype(np.float32))
