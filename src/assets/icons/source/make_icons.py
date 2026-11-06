# 列のアイコン10枚(24×24)を点で描く(詩織Ver6.0、2026-11-02。タスクはVer6.2)。
# 使い方: python make_icons.py <出力フォルダ>
# 色はペットの絵(src/assets/pet/idle.png)の27色から選ぶ。外側の輪郭線は付けない。
# 出力したPNGを、実機の data/ui/icons/ と src/assets/icons/ に置く。
import math,sys,os
from PIL import Image
C={'K':'181e48','N':'34448c','P':'808ec4','L':'b2c0e6','W':'dee6f7','S':'c8ccd8',
   'B':'b0843a','Y':'e2b860','D':'6e4c1e','G':'2c583a','g':'24685c','E':'183626',
   'M':'58c4a4','m':'b4e8d4','O':'ffa840','y':'ffe696','R':'c47070','r':'8a4a4a'}
def rgb(k):h=C[k];return tuple(int(h[i:i+2],16) for i in (0,2,4))+(255,)
class Ic:
    def __init__(s):s.im=Image.new('RGBA',(24,24),(0,0,0,0))
    def px(s,x,y,k):
        if 0<=x<24 and 0<=y<24:s.im.putpixel((x,y),rgb(k) if k else (0,0,0,0))
    def rect(s,x0,y0,x1,y1,k):
        for y in range(y0,y1+1):
            for x in range(x0,x1+1):s.px(x,y,k)
    def disc(s,cx,cy,r,k):
        for y in range(24):
            for x in range(24):
                if (x-cx)**2+(y-cy)**2<=r*r:s.px(x,y,k)
    def save(s,n):s.im.save(os.path.join(sys.argv[1],n+'.png'))
def star5(i,cx,cy,k):  # 5ドットの星
    for x,y in [(0,-2),(-1,-1),(0,-1),(1,-1),(-2,0),(-1,0),(0,0),(1,0),(2,0),(-1,1),(0,1),(1,1),(-1,2),(1,2)]:i.px(cx+x,cy+y,k)

# home: 屋根と、橙の灯りの窓
i=Ic()
for r in range(10):
    i.rect(11-r,2+r,12+r,2+r,'B')
for r in range(10):i.px(11-r,2+r,'Y')
i.rect(15,3,17,6,'P');i.rect(15,3,17,3,'L')
i.rect(4,12,19,21,'L');i.rect(16,12,19,21,'P');i.rect(4,21,19,21,'P')
i.rect(9,15,12,21,'D');i.px(11,18,'Y')
i.rect(14,14,17,17,'O');i.rect(14,14,17,14,'y');i.rect(15,15,16,16,'y');i.px(15,15,'O');i.px(16,16,'O')
i.save('home')

# conversation: 吹き出しと小さな星
i=Ic()
i.rect(3,4,20,15,'L');i.rect(2,5,21,14,'L');i.rect(3,15,20,15,'P');i.rect(21,6,21,14,'P')
i.rect(5,16,9,17,'L');i.rect(5,17,7,19,'L');i.px(5,19,'P');i.rect(8,16,9,16,'P')
i.rect(6,7,16,7,'N');i.rect(6,10,13,10,'N');i.rect(6,12,10,12,'P')
star5(i,19,4,'y')
i.save('conversation')

# tasks: 真鍮のクリップの紙と、苔色の印の並び(Ver6.2)
i=Ic()
i.rect(4,3,19,22,'B');i.rect(4,3,19,3,'Y');i.rect(4,3,4,22,'Y');i.rect(4,22,19,22,'D');i.rect(19,4,19,22,'D')
i.rect(6,6,17,20,'W');i.rect(6,20,17,20,'S');i.rect(17,6,17,20,'S')
i.rect(8,1,15,4,'P');i.rect(8,1,15,1,'L');i.rect(10,2,13,2,'N')
for y in (9,13,17):
    i.px(7,y,'g');i.px(8,y+1,'g');i.px(9,y,'g');i.px(10,y-1,'g')
    i.rect(12,y,15,y,'N' if y<17 else 'P')
i.save('tasks')

# library: 緑の革表紙の本と金の星
i=Ic()
i.rect(4,2,19,20,'g');i.rect(4,2,6,20,'E');i.rect(7,2,7,20,'M')
i.rect(19,3,19,20,'E');i.rect(4,20,19,20,'E')
i.rect(7,21,20,21,'W');i.rect(20,4,20,21,'W');i.rect(8,21,20,21,'L');i.rect(20,5,20,21,'L')
star5(i,13,10,'Y');i.px(13,10,'y');i.px(13,9,'y');i.px(12,10,'y');i.px(14,10,'y');i.px(13,11,'y')
i.save('library')

# library-browse: 札の並びと虫めがね
i=Ic()
for (x,y) in [(2,2),(12,2),(2,12)]:
    i.rect(x,y,x+8,y+7,'L');i.rect(x,y+7,x+8,y+7,'P');i.rect(x+2,y+2,x+6,y+2,'N');i.rect(x+2,y+4,x+5,y+4,'P')
i.disc(16,16,5,'Y');i.disc(16,16,3,'m');i.px(15,15,'W');i.px(14,16,'W')
for d in range(4):i.rect(19+d,19+d,20+d,20+d,'D')
i.save('library-browse')

# shelf: 本棚と並んだ背表紙
i=Ic()
i.rect(2,2,21,3,'B');i.rect(2,11,21,12,'B');i.rect(2,20,21,21,'B')
i.rect(2,2,3,21,'B');i.rect(20,2,21,21,'B')
i.rect(2,2,21,2,'Y');i.rect(2,11,21,11,'Y');i.rect(2,20,21,20,'Y')
i.rect(2,21,21,21,'D');i.rect(2,12,21,12,'D');i.rect(2,3,21,3,'D') if False else None
bk1=[('P',3),('M',2),('L',3),('g',2),('N',2),('Y',2)]
x=4
for k,w in bk1:
    i.rect(x,5,x+w-1,10,k);x+=w
    if x>19:break
i.rect(4,5,4,5,'W')
bk2=[('M',2),('P',2),('Y',3),('L',2),('g',3),('P',2)]
x=4
for k,w in bk2:
    i.rect(x,14,x+w-1,19,k);x+=w
    if x>19:break
i.save('shelf')

# claude: 4つの角の星のきらめき
i=Ic()
cx,cy=11.5,11.5
for y in range(24):
    for x in range(24):
        dx,dy=abs(x-cx),abs(y-cy)
        v=dx**0.5+dy**0.5
        if v<=10.8**0.5:i.px(x,y,'y')
        if v<=5.2**0.5:i.px(x,y,'W')
for (x,y) in [(3,3),(20,4),(20,19)]:
    i.px(x,y,'M');i.px(x-1,y,'M');i.px(x+1,y,'M');i.px(x,y-1,'M');i.px(x,y+1,'M')
i.save('claude')

# rules: 巻物と栞のひも
i=Ic()
i.rect(5,3,18,20,'W');i.rect(5,3,18,3,'S');i.rect(5,20,18,20,'S')
i.rect(3,2,20,4,'L');i.rect(3,19,20,21,'L');i.rect(3,4,20,4,'P');i.rect(3,21,20,21,'P')
i.rect(7,7,16,7,'N');i.rect(7,10,16,10,'P');i.rect(7,13,14,13,'N');i.rect(7,16,12,16,'P')
i.rect(15,15,17,21,'R');i.rect(15,22,17,22,'R');i.px(16,22,None);i.px(16,21,None);i.rect(15,15,15,22,'R');i.rect(17,15,17,22,'r')
i.save('rules')

# extensions: 真鍮のパズルのピース
i=Ic()
i.rect(3,9,15,21,'B');i.disc(9,6,3.4,'B');i.disc(18,15,3.4,'B')
i.rect(3,9,15,9,'Y');i.rect(3,9,3,21,'Y');i.rect(3,21,15,21,'D');i.rect(15,10,15,21,'D')
i.rect(7,3,10,3,'Y');i.px(6,4,'Y');i.px(6,5,'Y');i.rect(17,12,19,12,'Y');i.px(16,13,'Y');i.rect(20,15,20,17,'D');i.rect(17,18,19,18,'D')
i.save('extensions')

# settings: 歯車
i=Ic()
cx,cy=11.5,11.5
for y in range(24):
    for x in range(24):
        dx,dy=x-cx,y-cy;d=math.hypot(dx,dy);a=(math.degrees(math.atan2(dy,dx)))%45
        tooth=min(a,45-a)<12
        if d<=8.2 or (d<=10.6 and tooth):
            k='S'
            if dx+dy>5:k='P'
            elif dx+dy<-6:k='W'
            i.px(x,y,k)
        if 3.4<d<=5.2:i.px(x,y,'B')
        if d<=3.4:i.px(x,y,None)
i.save('settings')
