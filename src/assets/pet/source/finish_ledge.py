# 窓の縁での動き(飛び乗る・飛び降りる・縁に立つ・縁に座る・ぶら下がる・登る)の下絵を仕上げる
# 直すのは、ランプの穴、位置合わせ、ちらつきを抑える差し替えだけで、ほかの点は下絵のまま
# 使い方(このフォルダで。RAWは作業用のフォルダ):
#   python make_pose.py --pale ref/jump_on-20261104.webp 4.9 RAW/jump_on.png 74,421,250,990 552,913,90,930 1030,1439,5,840
#   python make_pose.py --pale --head-x=70 --white=0,803,262,1023 ref/jump_off-20261104.webp 4.7 RAW/jump_off.png 10,516,5,802 557,934,97,897 1035,1461,215,1001
#   python make_pose.py --pale ref/perch_stand-20261104.webp 5.33 RAW/perch_stand.png 94,471,8,1013 587,966,15,1013 1112,1470,16,1012
#   python make_pose.py --pale ref/perch_sit-20261104.webp 5.33 RAW/perch_sit.png 59,484,16,935 557,981,16,957 1057,1487,16,957
#   python make_pose.py --pale --face-from=14 ref/hang-20261104.webp 5.3 RAW/hang.png 107,393,0,1013 579,879,0,926 1067,1417,1,937
#   python make_pose.py --pale ref/climb-20261104.webp 4.9 RAW/climb.png 16,294,269,1009 378,716,161,915 774,1132,99,821
#   python make_pose.py --pale ref/climb-20261104.webp 4.05 RAW/climb4.png 1182,1499,0,759
#   python finish_ledge.py RAW ..
import sys
import numpy as np
from PIL import Image

from make_base import PAL

P = dict(PAL)
P['eyeL'] = (180, 232, 212)
P['faceLine'] = (118, 96, 96)
P['page'] = (236, 226, 200)
INV = {v: k for k, v in P.items()}

# brain.rs の定数と同じ値にする(手や足を、窓の縁に合わせるため)
HANG_TOP = 8    # ぶら下がるとき、窓の上辺が来る行
HANG_GRIP = 22  # ぶら下がるとき、窓の角が来る列(反転した絵の左から)
CLIMB_KEYS = [(0.15, 0.2), (0.45, 0.45), (0.52, 0.65), (1.0, 1.0)]  # 登るの姿ごとの動いた割合(横, 縦)

raw_dir, out_dir = sys.argv[1], sys.argv[2]


def load(name):
    a = np.array(Image.open(f'{raw_dir}/{name}.png').convert('RGBA'))
    return [a[:, i * 128:(i + 1) * 128].copy() for i in range(a.shape[1] // 128)]


def names_of(a):
    n = np.full(a.shape[:2], '', dtype=object)
    for y, x in zip(*np.where(a[..., 3] > 0)):
        n[y, x] = INV.get(tuple(int(v) for v in a[y, x, :3]), '?')
    return n


def fix_lamp(a):
    """ランプのガラスの中で、肌の色や透明になった点を、明るい灯りの色で埋める(持ち手の輪の中は隣に灯りがないので残る)"""
    n = names_of(a)
    fixed = []
    for y in range(1, 191):
        for x in range(1, 127):
            near = n[y - 1:y + 2, x - 1:x + 2]
            glow = int(np.isin(near, ['glow', 'glowL']).sum())
            if n[y, x] in ('skin', 'skinD', 'star', 'hairL', 'page') and glow >= 3:
                fixed.append((y, x))
            elif n[y, x] == '' and glow >= 2:
                sides = [a[y + dy * t, x + dx * t, 3] > 0 for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)) for t in (1, 2)
                         if 0 <= y + dy * t < 192 and 0 <= x + dx * t < 128]
                if sum(sides) >= 6:
                    fixed.append((y, x))
    for y, x in fixed:
        a[y, x, :3] = P['glowL']
        a[y, x, 3] = 255
    return a


def bbox(a):
    ys, xs = np.where(a[..., 3] > 0)
    return xs.min(), xs.max(), ys.min(), ys.max()


def moved(a, dx, dy):
    """絵をずらす。枠からはみ出すなら止める(点が欠けないように)"""
    x0, x1, y0, y1 = bbox(a)
    if x0 + dx < 0 or x1 + dx > 127 or y0 + dy < 0 or y1 + dy > 191:
        raise SystemExit(f'はみ出し: {x0 + dx}-{x1 + dx}, {y0 + dy}-{y1 + dy}')
    out = np.zeros_like(a)
    out[max(dy, 0):192 + min(dy, 0), max(dx, 0):128 + min(dx, 0)] = a[max(-dy, 0):192 + min(-dy, 0), max(-dx, 0):128 + min(-dx, 0)]
    return out


def save(name, seq):
    Image.fromarray(np.concatenate(seq, axis=1), 'RGBA').save(f'{out_dir}/{name}.png')
    print(name, len(seq), 'コマ')


# 飛び乗る・飛び降りる: 踏み切り→途中→最後の姿のまま(跳ぶ時間は0.45秒から。長いときは最後の姿で止まる)
for name in ('jump_on', 'jump_off'):
    p = [fix_lamp(f) for f in load(name)]
    save(name, [p[0], p[1], p[2], p[2]])

# 縁に立つ: 足元の位置をそろえて、左→下→右→下とゆっくり見る(4秒で繰り返す)
p = [fix_lamp(f) for f in load('perch_stand')]


def boots_x(a):
    ys, xs = np.where(np.isin(names_of(a), ['boot', 'bootD', 'bootL']))
    return xs.mean()


p = [moved(f, int(round(boots_x(p[1]) - boots_x(f))), 0) for f in p]
save('perch_stand', [p[0]] * 10 + [p[1]] * 6 + [p[2]] * 10 + [p[1]] * 6)

# 縁に座る: 体は1番に固定し、裾から下の脚だけを2番・3番のものに差し替える
p = [fix_lamp(f) for f in load('perch_sit')]
LEGS = ['skin', 'skinD', 'boot', 'bootD', 'bootL']
SWING_FROM = 128   # 脚を差し替える行(本より下。ランプと手は x<46 なので入らない)
# 元の絵の上端がそろっていて足元だけ違うので、下端で置いた絵を上端でそろえ直す
tops = [bbox(f)[2] for f in p]
p = [moved(f, 0, tops[1] - t) for f, t in zip(p, tops)]


def leg_mask(a):
    n = names_of(a)
    m = np.isin(n, LEGS)
    # 脚の輪郭(すぐ隣の濃い線と、薄い線に寄った色)も一緒に動かす。裾の模様は離れているので動かない
    grown = m.copy()
    for y, x in zip(*np.where(np.isin(n, ['line', 'faceLine', 'hairD']))):
        if m[max(y - 2, 0):y + 3, max(x - 2, 0):x + 3].any():
            grown[y, x] = True
    grown[:SWING_FROM] = False
    grown[:, :46] = False
    return grown


base = p[0].copy()
base[leg_mask(p[0])] = 0
sit = [p[0]]
for k in (1, 2):
    f = base.copy()
    m = leg_mask(p[k])
    f[m] = p[k][m]
    sit.append(f)
# しばらく静かに座り、足をゆっくり2回振る
swing = [sit[1], sit[1], sit[0], sit[0], sit[2], sit[2], sit[0], sit[0]]
save('perch_sit', [sit[0]] * 16 + swing + swing)

# ぶら下がる: 手の高さと位置を3枚でそろえる。手の中ほどを窓の角の少し内側に置く
p = [fix_lamp(f) for f in load('hang')]
HAND_X = 127 - HANG_GRIP - 5
out = []
for f in p:
    x0, x1, y0, y1 = bbox(f)
    xs = np.where(f[y0:y0 + 8, :, 3].any(axis=0))[0]
    out.append(moved(f, int(round(HAND_X - xs.mean())), HANG_TOP - 5 - y0))
# 元の絵は、まっすぐ→足が後ろへ→さらに後ろへ、の片側の揺れなので、行って戻る順に流す
save('hang', [out[0]] * 8 + [out[1]] * 2 + [out[2]] * 4 + [out[1]] * 2)

# 登る: 窓の角(元の絵の座標)が、そのコマの窓の位置に来るように置く
p = [fix_lamp(f) for f in load('climb')]
RATIO = 4.9
EDGES = [(16, 989, 255, 478),(378, 895, 605, 506), (774, 801, 1007, 505)]  # 姿の左端x, 下端y, 角x, 角y
out = []
for f, (x0, y1, cx, cy), (kx, ky) in zip(p, EDGES, CLIMB_KEYS):
    col = 127 - (HANG_GRIP + (128 - HANG_GRIP) * kx)
    row = HANG_TOP + (192 - HANG_TOP) * ky
    bx0, _, _, by1 = bbox(f)
    out.append(moved(f, int(round(col - (cx - x0) / RATIO - bx0)), int(round(row + (y1 - cy) / RATIO - by1))))
# 4番は立ち姿なので普段の背丈の比で作った絵を、そのまま使う
out.append(fix_lamp(load('climb4')[0]))
save('climb', [out[0], out[0], out[1], out[1], out[2], out[2], out[3], out[3]])
