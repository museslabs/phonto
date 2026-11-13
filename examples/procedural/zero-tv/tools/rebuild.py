# Builds back.png, the back of the shelf of zerotv.mp4 without the tapes, for
# the Zero TV pack. The pack draws the tapes as graphics in front of it.
# Usage: rebuild.py ASSETS PACK. ASSETS holds mean.png and std.npy from
# average.py.
#
# In the video, tape 1 is selected: it is 18 pixels lower, with a taller top
# face. First, the script finds every tape as it is at rest. It moves tape 1
# up to its place at rest, and it completes the part of tape 2 that tape 1
# covers.
# Then it removes every tape, with its outline and its top face, and it fills
# the back of the shelf behind them. Above the floor, that is the curtain. Near
# the floor, each row blends its visible pixels.
import sys
import numpy as np
from PIL import Image
from collections import deque

A, P = sys.argv[1], sys.argv[2]
X0, Y0, W, H = 120, 760, 1640, 280
mean = np.asarray(Image.open(f"{A}/mean.png")).astype(float)
std = np.load(f"{A}/std.npy")
PULL = 18          # how far a selected tape comes down

def at(img, x, y):
    return img[y - Y0, x - X0]

def region(img, x0, y0, x1, y1):
    return img[y0 - Y0:y1 - Y0, x0 - X0:x1 - X0]

shelf = mean.copy()

# --- Tape 1 at rest -------------------------------------------------------
# The selected tape 1: top face 782-800, front face 800-861, x 172-648.
T1X0, T1X1 = 172, 648
moving = std > 2.5                       # the pixels of tape 1 pulse
# Clear the selected tape to the curtain behind it. The glow of the pulse
# blurs past the ends of the tape (x 179-640), into tape 2 and the
# background. So the tape is only the pixels that pulse between its ends.
selected = np.zeros((H, W), bool)
selected[782 - Y0:862 - Y0, 179 - X0:641 - X0] = region(moving, 179, 782, 641, 862)
for y, x in zip(*np.nonzero(selected)):
    shelf[y, x] = mean[765 - Y0, x]
# The video shows a faint glow directly left and right of the tape, above
# tape 2 (row 844). Those strips take the background of their row.
for y in range(782, 844):
    shelf[y - Y0, 160 - X0:179 - X0] = mean[y - Y0, 150 - X0]
    shelf[y - Y0, 641 - X0:652 - X0] = mean[y - Y0, 656 - X0]
# Up by PULL: the front face at rest is 782-843, as tapes 5 and 9.
for y in range(800, 862):
    for x in range(T1X0, T1X1):
        if selected[y - Y0, x - X0]:
            shelf[y - PULL - Y0, x - X0] = at(mean, x, y)

# The top face of tape 1, from the video, 18 rows (782-799). Each row covers
# the whole width between its ends.
top1_alpha = region(moving, T1X0, 782, T1X1, 800).copy()
cols = np.nonzero(top1_alpha.any(axis=0))[0]
top1_alpha[:, cols[0]:cols[-1] + 1] = True

# --- Tape 2: the rows that tape 1 covers (843-864, x 179-648) -------------
# Tape 6 is in the same row of its stack. Map its columns at the same parts
# of the tape: left end, label left, label right, notch, right end.
t2 = [158, 226, 546, 562, 646]
t6 = [714, 782, 1100, 1112, 1153]
for y in range(843, 865):
    for x in range(179, 647):
        sx = np.interp(x, t2, t6)
        x6 = int(round(sx))
        shelf[y - Y0, x - X0] = at(mean, x6, y)
    # Past the right end of tape 2, the curtain.
    for x in range(647, 652):
        shelf[y - Y0, x - X0] = mean[765 - Y0, x - X0]

# --- The tapes: front-face masks -----------------------------------------
TAPES = {1: (179, 782, 640, 843), 2: (157, 843, 643, 905), 3: (133, 905, 646, 966), 4: (150, 966, 640, 1028),
         5: (726, 782, 1170, 843), 6: (713, 843, 1152, 905), 7: (742, 905, 1177, 967), 8: (742, 967, 1160, 1028),
         9: (1255, 782, 1725, 843), 10: (1253, 843, 1728, 905), 11: (1236, 905, 1749, 967), 12: (1245, 967, 1742, 1028)}
lum = shelf.mean(axis=2)

# The search for a tape reaches this far past its rectangle, so the soft
# outline at its ends belongs to it.
WIDEN = 5

def flood_mask(k):
    x0, y0, x1, y1 = TAPES[k]
    x0, x1 = x0 - WIDEN, x1 + WIDEN
    reg = lum[y0 - Y0:y1 - Y0, x0 - X0:x1 - X0]
    dark = reg < 42
    h, w = dark.shape
    bg = np.zeros_like(dark)
    q = deque()
    for y in range(h):
        for x in (0, w - 1):
            if dark[y, x] and not bg[y, x]: bg[y, x] = True; q.append((y, x))
    for x in range(w):
        for y in (0, h - 1):
            if dark[y, x] and not bg[y, x]: bg[y, x] = True; q.append((y, x))
    while q:
        y, x = q.popleft()
        for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            ny, nx = y + dy, x + dx
            if 0 <= ny < h and 0 <= nx < w and dark[ny, nx] and not bg[ny, nx]:
                bg[ny, nx] = True; q.append((ny, nx))
    g = ~bg
    for _ in range(2):
        n = g.copy()
        n[1:] |= g[:-1]; n[:-1] |= g[1:]; n[:, 1:] |= g[:, :-1]; n[:, :-1] |= g[:, 1:]
        g = n
    for row in g:
        xs = np.nonzero(row)[0]
        if len(xs):
            row[xs[0]:xs[-1] + 1] = True
    # The dark line between two tapes is the top outline of the lower tape,
    # so it comes out with that tape.
    for x in range(g.shape[1]):
        ys = np.nonzero(g[:8, x])[0]
        if len(ys):
            g[:ys[0], x] = True
    return g

ids = np.zeros((H, W), np.uint8)
for k, (x0, y0, x1, y1) in TAPES.items():
    x0, x1 = x0 - WIDEN, x1 + WIDEN
    m = flood_mask(k)
    view = ids[y0 - Y0:y1 - Y0, x0 - X0:x1 - X0]
    view[m & (view == 0)] = k

# --- Top faces of tapes 5 and 9 (770-781), with their side faces ----------
BG = np.array([18, 5, 38], float)
def top_strip(x0, x1):
    alpha = np.abs(region(mean, x0, 770, x1, 782) - BG).max(axis=2) > 10
    for row in alpha:
        xs = np.nonzero(row)[0]
        if len(xs):
            row[xs[0]:xs[-1] + 1] = True
    return alpha
top5_alpha = top_strip(720, 1220)
top9_alpha = top_strip(1248, 1760)

# --- The top face of each tape at rest ------------------------------------
# `tops` holds a top face of 18 rows for each tape: True where the face is.
# At rest, a top face is top_rest(k) rows tall. `rest_alpha` is where the top
# faces cover the shelf at rest, so that the back of the shelf fills those
# pixels too.
TOP_ROWS = 18
def top_rest(k):
    return 12 if k in (1, 5, 9) else 16

def front_ends(k):
    x0, y0, x1, y1 = TAPES[k]
    xs = np.nonzero((ids[y0 - Y0:y0 - Y0 + 4] == k).any(axis=0))[0]
    return int(xs[0] + X0), int(xs[-1] + 1 + X0)

col_above = mean[765 - Y0]
def is_background(c, x):
    return np.abs(c - col_above[x - X0]).max() <= 12

tops = np.zeros((12 * TOP_ROWS, W), bool)
rest_alpha = np.zeros((H, W), bool)
for k, (x0, y0, x1, y1) in TAPES.items():
    base = (k - 1) * TOP_ROWS
    rest = top_rest(k)
    if k == 1:
        tops[base:base + 18, T1X0 - X0:T1X1 - X0] = top1_alpha
    elif k in (5, 9):
        alpha, sx0 = (top5_alpha, 720) if k == 5 else (top9_alpha, 1248)
        for r in range(TOP_ROWS):
            i = (r * 2 + 1) * rest // (TOP_ROWS * 2)
            tops[base + r, sx0 - X0:sx0 - X0 + alpha.shape[1]] = alpha[i]
    else:
        # The top face of tape 1, fitted to the width of this tape.
        f0, f1 = front_ends(k)
        scale = (f1 - f0) / 465.0
        for x in range(f0 - 4, f1 + 4):
            sx = int(round(177 + (x - f0) / scale)) - T1X0
            if 0 <= sx < top1_alpha.shape[1] and top1_alpha[:, sx].any():
                tops[base:base + 18, x - X0] = True
        # In a column that the tape above does not hide, the video shows this
        # top face, or the background past its end. There, the top face is
        # where the video shows the tape, and not where it shows the
        # background.
        above = k - 1
        for x in range(f0 - 8, f1 + 8):
            col = [y0 - rest + i for i in range(rest)]
            if all(ids[y - Y0, x - X0] == above for y in col):
                continue
            for r in range(TOP_ROWS):
                i = (r * 2 + 1) * rest // (TOP_ROWS * 2)
                y = y0 - rest + i
                if ids[y - Y0, x - X0] == above:
                    continue
                pixel = shelf[y - Y0, x - X0]
                tops[base + r, x - X0] = ids[y - Y0, x - X0] == 0 and not is_background(pixel, x)
    for i in range(rest):
        r = (i * 2 + 1) * TOP_ROWS // (rest * 2)
        rest_alpha[y0 - rest + i - Y0] |= tops[base + r]

# --- The shelf behind the tapes ------------------------------------------
# Where the video shows the back of the shelf, keep it. Above the floor, the
# curtain hangs straight, so a hidden pixel takes the color of its column
# directly above the stacks. From row 912 down are the back edge of the floor
# and the floor, which run straight across. There, a hidden pixel takes the
# color of its row in the two gaps between the stacks. It blends from the
# left gap to the right gap.
visible = (ids == 0) & ~rest_alpha
back = shelf.copy()
left_gap = mean[:, 660 - X0:705 - X0].mean(axis=1)
right_gap = mean[:, 1185 - X0:1230 - X0].mean(axis=1)
for y in range(H):
    for x in range(W):
        if visible[y, x]:
            continue
        if y + Y0 < 912:
            back[y, x] = col_above[x]
        else:
            t = min(max((x + X0 - 683) / (1207 - 683), 0.0), 1.0)
            back[y, x] = left_gap[y] * (1 - t) + right_gap[y] * t
# At rest, the top face of tape 1 covers rows 770-781. The video shows the
# background there, so the back of the shelf keeps it.
back[770 - Y0:782 - Y0, T1X0 - X0:T1X1 - X0] = col_above[T1X0 - X0:T1X1 - X0]

Image.fromarray(np.clip(back + .5, 0, 255).astype(np.uint8)).save(f"{P}/back.png", optimize=True)
