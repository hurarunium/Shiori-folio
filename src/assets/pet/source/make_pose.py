# 生成AIの姿の絵(姿を横に並べた1枚)から、姿ごとの下絵を作る。make_base.py と同じ作り方
# (面積平均で縮める → 白地を背景として2値に分ける → 決めた色に寄せる)。手直しはしない
# 使い方: python make_pose.py <元の絵> <縮める比(元の画素/1ドット)> <出力のコマ並びPNG> x0,x1[,y0,y1] ...
# 今の絵の作り方(このフォルダで):
#   座って読む(比は、立ち姿の背丈548画素を180ドットにする3.044):
#     python make_pose.py ref/sit-read-20261024.webp 3.044 read_raw.png 16,224,10,570 275,533,10,570 561,886,10,570 889,1190,10,570 1206,1514,10,570 220,551,590,1010 566,904,590,1010 922,1265,590,1010
#   フードをかぶって眠る(比は、座った姿の高さを、座って読むの3番の姿に合わせた4.257):
#     python make_pose.py ref/hood-sleep-20261024.webp 4.257 sleep_raw.png 21,446,150,760 471,876,150,760 899,1299,150,760 1331,1749,150,760
#   窓の縁での6つの動きの作り方は finish_ledge.py の先頭に書いた
import sys
import numpy as np
from PIL import Image

from make_base import PAL

PAL = dict(PAL)
PAL['eyeL'] = (180, 232, 212)
PAL['faceLine'] = (118, 96, 96)
PAL['page'] = (236, 226, 200)   # 開いた本のページ(読むの姿で足した色)

FW, FH = 128, 192
FOOT = 190      # 床(足元・裾の下端)のy
HEAD_X = 64     # 頭のてっぺんの中ほどを置くx

# 2026-11-04の絵(飛び乗るなど)の指定。肌が前の絵より白く、白地や星の色に寄るので、暖かい白を肌に寄せる。
# 外套の明るさも前の絵より高いので、明るい青は外套の色に寄せる。--face-from=N は顔を探し始める行(上端からのドット)
PALE = '--pale' in sys.argv
FACE_FROM = next((int(a.split('=')[1]) for a in sys.argv if a.startswith('--face-from=')), 0)
# --head-x=N は頭のてっぺんの中ほどを置くx。--white=x0,y0,x1,y1 は元の絵に描かれてしまった台などを白で消す範囲
HEAD_X = next((int(a.split('=')[1]) for a in sys.argv if a.startswith('--head-x=')), HEAD_X)
WHITE = next((tuple(int(v) for v in a.split('=')[1].split(',')) for a in sys.argv if a.startswith('--white=')), None)
sys.argv = [a for a in sys.argv if not a.startswith('--')]

src = Image.open(sys.argv[1]).convert('RGB')
if WHITE:
    src.paste((255, 255, 255), WHITE)
ratio = float(sys.argv[2])
out = sys.argv[3]
boxes = [tuple(int(v) for v in b.split(',')) for b in sys.argv[4:]]

EYE_PAL = ['line', 'eyeD', 'eye', 'eyeL', 'skin', 'skinD', 'faceLine', 'hairD']


def largest_blob(m):
    """上下左右につながった、いちばん大きなかたまりだけを残す"""
    seen = np.zeros(m.shape, bool)
    best = []
    for sy, sx in zip(*np.where(m)):
        if seen[sy, sx]:
            continue
        seen[sy, sx] = True
        stack, blob = [(sy, sx)], []
        while stack:
            y, x = stack.pop()
            blob.append((y, x))
            for ny, nx in ((y + 1, x), (y - 1, x), (y, x + 1), (y, x - 1)):
                if 0 <= ny < m.shape[0] and 0 <= nx < m.shape[1] and m[ny, nx] and not seen[ny, nx]:
                    seen[ny, nx] = True
                    stack.append((ny, nx))
        if len(blob) > len(best):
            best = blob
    out = np.zeros(m.shape, bool)
    for y, x in best:
        out[y, x] = True
    return out


names = list(PAL)
pv = np.array([PAL[n] for n in names], float)
wt = np.array([0.35, 0.45, 0.2])

full = np.array(src).astype(int)
fg_full = full.min(axis=2) <= 235
frames = []
for b in boxes:
    x0, x1 = b[0], b[1]
    col_fg = fg_full[:, x0:x1 + 1]
    rows = np.where(col_fg.any(axis=1))[0]
    y0, y1 = (b[2], b[3]) if len(b) == 4 else (rows.min(), rows.max())
    crop = src.crop((x0, y0, x1 + 1, y1 + 1))
    W = round(crop.width / ratio)
    H = round(crop.height / ratio)
    a = np.array(crop).astype(float)
    bg_px = a.min(axis=2) > 235
    if PALE:
        bg_px &= (a[:, :, 0] - a[:, :, 2]) < 5   # 暖かい白は肌なので背景にしない
    bg = bg_px.astype(np.uint8) * 255
    mask = np.array(Image.fromarray(bg).resize((W, H), Image.BOX)) < 110
    col = np.array(crop.resize((W, H), Image.BOX)).astype(float)
    d = (((col[:, :, None, :] - pv[None, None]) ** 2) * wt).sum(axis=3)
    idx = d.argmin(axis=2)
    if PALE:
        warm = (col[:, :, 0] - col[:, :, 2] >= 6) & (col.min(axis=2) > 195)
        idx[warm] = names.index('skin')
        idx[(idx == names.index('robeL')) & (col[:, :, 2] < 128)] = names.index('robe')
    # 顔の中(頭のてっぺんから44ドットまでにある肌の色の点の範囲)で、本・ブーツ・真鍮などの色に寄った点は、
    # 目に使う色だけから選び直す(前回の顔の直しと同じ。まつげが別の部品の色に寄る潰れ方)。
    # 同じ縮尺なので、顔は頭のてっぺんから44ドットに収まり、本を持つ手はそれより下にある
    top0 = np.where(mask.any(axis=1))[0].min()
    skin = np.isin(idx, [names.index('skin'), names.index('skinD')]) & mask
    skin[:top0 + FACE_FROM] = False
    skin[top0 + FACE_FROM + 44:] = False
    if PALE:
        # 手が顔の範囲に入っても外套まで直さないよう、いちばん大きな肌のかたまり(顔)だけを見る
        skin = largest_blob(skin)
    sy, sx = np.where(skin)
    best = (0, sy.min(), sy.max(), sx.min(), sx.max()) if len(sy) else None
    if best:
        _, fy0, fy1, fx0, fx1 = best
        bad = [names.index(n) for n in ('book', 'bookD', 'boot', 'bootD', 'bootL', 'brassD', 'brass', 'brassL', 'gold', 'robeD', 'robe')]
        ep = np.array([PAL[n] for n in EYE_PAL], float)
        for yy in range(fy0, fy1 + 1):
            for xx in range(fx0, fx1 + 1):
                if mask[yy, xx] and idx[yy, xx] in bad:
                    k = int((((ep - col[yy, xx]) ** 2) * wt).sum(axis=1).argmin())
                    idx[yy, xx] = names.index(EYE_PAL[k])
    small = np.zeros((H, W, 4), np.uint8)
    small[..., :3] = pv[idx].astype(np.uint8)
    small[..., 3] = np.where(mask, 255, 0)
    # 頭のてっぺん(上から8ドット)の中ほどをHEAD_Xにそろえ、下端を床に置く
    ys, xs = np.where(mask)
    top = ys.min()
    hx = xs[ys < top + 8].mean()
    ox = int(round(HEAD_X - hx))
    oy = FOOT - (ys.max() + 1) + 1
    canvas = np.zeros((FH, FW, 4), np.uint8)
    for yy in range(H):
        for xx in range(W):
            X, Y = xx + ox, yy + oy
            if small[yy, xx, 3] and 0 <= X < FW and 0 <= Y < FH:
                canvas[Y, X] = small[yy, xx]
    print(f'姿 {x0}-{x1}: {W}x{H}ドット 置いた位置 x{ox} y{oy} はみ出し {"あり" if ox < 0 or ox + W > FW or oy < 0 else "なし"}')
    frames.append(canvas)

Image.fromarray(np.concatenate(frames, axis=1), 'RGBA').save(out)
