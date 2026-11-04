//! ペットの世界と足場(ペットの行動の設計書 `design-shiori-pet-behavior-20261103` の2節)。
//!
//! 全部の画面をつなげた1つの世界で、ペットが立てる水平な線分(足場)を計算する。足場は、
//! 画面ごとの床(作業領域の下端)と、ほかのウィンドウに隠れていないウィンドウの上辺。
//! 座標はペットと同じ単位(Windowsは物理の画素、Macは論理の点)で受け取る。
//! tauriにもOSにも依存させない(Windowsでもこのファイルだけを小さなクレートでテストできるように)。

use serde::Serialize;

/// 左上と大きさ(x, y, 幅, 高さ)
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f64 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }

    /// 点を含むか(右と下の辺は含まない)
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

/// 画面(画面全体と作業領域)
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct ScreenArea {
    pub full: Rect,
    pub work: Rect,
}

/// ほかのアプリのウィンドウ。一覧は前面から奥への順に並べる
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WinInfo {
    /// Windows: HWND、Mac: ウィンドウの番号
    pub id: u64,
    /// アプリ名(Windowsは実行ファイル名、Macはアプリの名前)。タイトルは読まない
    pub app: String,
    pub rect: Rect,
    /// 前面(入力を受けている)ウィンドウか
    pub focused: bool,
}

/// 前に一覧にあったウィンドウが、いまどうなっているか(`windows::presence` が返す)
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Presence {
    /// 画面に出ている(位置と大きさ)
    Visible(Rect),
    /// 残っているが見えない(別の仮想デスクトップ。Macでは最小化もここに入る。区別する方法が無いため)
    Hidden,
    /// 最小化した(Windowsだけ。Macは見えないと区別できないので作らない)
    #[cfg_attr(not(windows), allow(dead_code))]
    Minimized,
    /// 閉じた
    Gone,
}

/// 足場の持ち主
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Owner {
    /// 画面の床(画面の一覧の番号)
    Floor { screen: usize },
    Window { id: u64 },
}

/// ペットが立てる水平な線分(足の高さ y、左右 x0〜x1)
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Platform {
    pub owner: Owner,
    pub y: f64,
    pub x0: f64,
    pub x1: f64,
    /// 計算したときの持ち主の左端(ウィンドウの左。床は0)。ウィンドウが動いたあとも、
    /// 足場の見えている部分をウィンドウからの距離で見られるように持つ
    pub anchor: f64,
}

/// 幅がペットのこの倍より狭いウィンドウは足場にしない
const MIN_WIDTH_RATIO: f64 = 1.5;

/// 足場を計算する。`pet` はペットの絵の幅と高さ。`ride_windows` がfalseなら床だけ
/// (ウィンドウに乗る行動をオフにしたとき。設計書の6節)。
pub fn platforms(screens: &[ScreenArea], windows: &[WinInfo], pet: (f64, f64), ride_windows: bool) -> Vec<Platform> {
    let (pw, ph) = pet;
    let mut out = Vec::new();
    for (i, s) in screens.iter().enumerate() {
        out.push(Platform {
            owner: Owner::Floor { screen: i },
            y: s.work.bottom(),
            x0: s.work.x,
            x1: s.work.right(),
            anchor: 0.0,
        });
    }
    if !ride_windows {
        return out;
    }
    for (i, w) in windows.iter().enumerate() {
        if w.rect.w < pw * MIN_WIDTH_RATIO {
            continue;
        }
        let y = w.rect.y;
        let mut segs = vec![(w.rect.x, w.rect.right())];
        // 前にあるウィンドウに隠れた部分を除く
        for front in &windows[..i] {
            if front.rect.y <= y && front.rect.bottom() > y {
                segs = subtract(&segs, (front.rect.x, front.rect.right()));
            }
        }
        // 足もとと頭が、どれかの画面の作業領域に入る部分だけ(縦に並べた画面なら、上の画面に頭が出てもよい)
        segs = intersect(&segs, &covered(screens, y));
        segs = intersect(&segs, &covered(screens, y - ph));
        for (x0, x1) in segs {
            if x1 - x0 >= pw {
                out.push(Platform { owner: Owner::Window { id: w.id }, y, x0, x1, anchor: w.rect.x });
            }
        }
    }
    out
}

/// 行き先を選ぶ画面で押した点から、向かう足場と、着いたときの絵の左上のxを決める(設計書の9節)。
/// 乗れるウィンドウの中ならその上辺、無ければ点の下でいちばん近い足場、それも無ければその画面の床
pub fn pick_target(
    screens: &[ScreenArea],
    windows: &[WinInfo],
    platforms: &[Platform],
    pet_w: f64,
    x: f64,
    y: f64,
) -> Option<(Platform, f64)> {
    // 上辺が隠れて分かれているときは、点の真上に近い部分
    let gap = |p: &Platform| (p.x0 - x).max(x - p.x1).max(0.0);
    let on_window = windows.iter().find(|w| w.rect.contains(x, y)).and_then(|w| {
        platforms.iter().filter(|p| p.owner == Owner::Window { id: w.id }).min_by(|a, b| gap(a).total_cmp(&gap(b)))
    });
    let below = || platforms.iter().filter(|p| p.y >= y && x >= p.x0 && x <= p.x1).min_by(|a, b| a.y.total_cmp(&b.y));
    let floor = || {
        let screen = screen_index_at(screens, x, y)?;
        platforms.iter().find(|p| p.owner == Owner::Floor { screen })
    };
    let p = *on_window.or_else(below).or_else(floor)?;
    Some((p, (x - pet_w / 2.0).clamp(p.x0, (p.x1 - pet_w).max(p.x0))))
}

/// 点を含む画面の番号(画面全体で見る)
pub fn screen_index_at(screens: &[ScreenArea], x: f64, y: f64) -> Option<usize> {
    screens.iter().position(|s| s.full.contains(x, y))
}

/// 前面のウィンドウが、どれかの画面全体を覆っているか(全画面のアプリ。設計書の5節)
pub fn fullscreen_screen(screens: &[ScreenArea], windows: &[WinInfo]) -> Option<usize> {
    let w = windows.iter().find(|w| w.focused)?;
    screens.iter().position(|s| {
        w.rect.x <= s.full.x && w.rect.y <= s.full.y && w.rect.right() >= s.full.right() && w.rect.bottom() >= s.full.bottom()
    })
}

/// 高さ y の水平線のうち、どれかの画面の作業領域に入る部分(重なりはつなげる)
fn covered(screens: &[ScreenArea], y: f64) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> =
        screens.iter().filter(|s| y >= s.work.y && y <= s.work.bottom()).map(|s| (s.work.x, s.work.right())).collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (a, b) in v {
        match out.last_mut() {
            // 隣り合う画面(右端と左端が同じ)もつなげる
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

fn subtract(segs: &[(f64, f64)], cut: (f64, f64)) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a, b) in segs {
        if cut.1 <= a || cut.0 >= b {
            out.push((a, b));
            continue;
        }
        if cut.0 > a {
            out.push((a, cut.0));
        }
        if cut.1 < b {
            out.push((cut.1, b));
        }
    }
    out
}

fn intersect(segs: &[(f64, f64)], with: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for &(a, b) in segs {
        for &(c, d) in with {
            let (lo, hi) = (a.max(c), b.min(d));
            if hi > lo {
                out.push((lo, hi));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ペットの絵の幅と高さ(128×192ドットを1倍で)
    const PET: (f64, f64) = (128.0, 192.0);

    /// 横置きの画面(下に40のタスクバー)
    fn screen(x: f64, y: f64, w: f64, h: f64) -> ScreenArea {
        ScreenArea { full: Rect::new(x, y, w, h), work: Rect::new(x, y, w, h - 40.0) }
    }

    fn win(id: u64, x: f64, y: f64, w: f64, h: f64) -> WinInfo {
        WinInfo { id, app: format!("app{id}"), rect: Rect::new(x, y, w, h), focused: id == 1 }
    }

    #[test]
    fn 行き先を選ぶ_ウィンドウの中_その下の足場_床の順() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0)];
        // 2は幅が狭いので足場にならない
        let wins = [win(1, 400.0, 500.0, 600.0, 300.0), win(2, 1200.0, 500.0, 100.0, 300.0)];
        let ps = platforms(&screens, &wins, PET, true);
        let pick = |x, y| pick_target(&screens, &wins, &ps, PET.0, x, y).map(|(p, at)| (p.owner, at));
        // ウィンドウの中は、その上辺。絵の真ん中が点に来る
        assert_eq!(pick(700.0, 650.0), Some((Owner::Window { id: 1 }, 636.0)));
        // ウィンドウの上の何も無い所は、その下でいちばん近い足場
        assert_eq!(pick(700.0, 100.0), Some((Owner::Window { id: 1 }, 636.0)));
        // 足場にならないウィンドウの中は、その下の足場(床)
        assert_eq!(pick(1250.0, 650.0), Some((Owner::Floor { screen: 0 }, 1186.0)));
        // 床より下(画面の下の帯)は、その画面の床
        assert_eq!(pick(1500.0, 1060.0), Some((Owner::Floor { screen: 0 }, 1436.0)));
        // 端では、絵が足場からはみ出ない
        assert_eq!(pick(405.0, 650.0), Some((Owner::Window { id: 1 }, 400.0)));
        // どの画面にも無い点
        assert_eq!(pick(-500.0, 5000.0), None);
    }

    fn of(ps: &[Platform], id: u64) -> Vec<(f64, f64, f64)> {
        ps.iter().filter(|p| p.owner == Owner::Window { id }).map(|p| (p.y, p.x0, p.x1)).collect()
    }

    #[test]
    fn 床は画面ごとに作業領域の下端() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0), screen(1920.0, 0.0, 1920.0, 1080.0)];
        let ps = platforms(&screens, &[], PET, true);
        assert_eq!(ps.len(), 2);
        assert_eq!(ps[0], Platform { owner: Owner::Floor { screen: 0 }, y: 1040.0, x0: 0.0, x1: 1920.0, anchor: 0.0 });
        assert_eq!(ps[1].x0, 1920.0);
    }

    #[test]
    fn 前のウィンドウに隠れた上辺は除く() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0)];
        // 奥のウィンドウ(2)の上辺 y=400 の真ん中を、前のウィンドウ(1)が覆う
        let ws = [win(1, 700.0, 300.0, 400.0, 400.0), win(2, 200.0, 400.0, 1200.0, 500.0)];
        let ps = platforms(&screens, &ws, PET, true);
        assert_eq!(of(&ps, 2), vec![(400.0, 200.0, 700.0), (400.0, 1100.0, 1400.0)]);
        // 前のウィンドウ自身の上辺は、そのまま
        assert_eq!(of(&ps, 1), vec![(300.0, 700.0, 1100.0)]);
    }

    #[test]
    fn 上辺が前のウィンドウの下より下なら隠れない() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0)];
        let ws = [win(1, 0.0, 200.0, 1920.0, 200.0), win(2, 300.0, 400.0, 800.0, 300.0)];
        let ps = platforms(&screens, &ws, PET, true);
        assert_eq!(of(&ps, 2), vec![(400.0, 300.0, 1100.0)]);
    }

    #[test]
    fn 細いウィンドウと_ペットより短い見えている部分は足場にしない() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0)];
        let ws = [win(1, 400.0, 500.0, 150.0, 300.0), win(2, 300.0, 500.0, 300.0, 300.0)];
        let ps = platforms(&screens, &ws, PET, true);
        // 1は幅150 < 128×1.5。2は見えている部分が300〜400と550〜600で、どちらもペットより短い
        assert!(of(&ps, 1).is_empty());
        assert!(of(&ps, 2).is_empty());
    }

    #[test]
    fn 画面1枚で最大化したウィンドウは_頭がはみ出るので足場にしない() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0)];
        let ws = [win(1, 0.0, 0.0, 1920.0, 1040.0), win(2, 100.0, 100.0, 800.0, 600.0)];
        let ps = platforms(&screens, &ws, PET, true);
        assert!(of(&ps, 1).is_empty());
        // y=100では頭(100-192)が画面の上に出る
        assert!(of(&ps, 2).is_empty());
    }

    #[test]
    fn 縦に並べた画面なら_下の画面で最大化したウィンドウにも乗れる() {
        // 上の画面(iPadなど)の下端と、下の画面の上端がつながる。上の画面には床のタスクバーが無い
        let up = ScreenArea { full: Rect::new(0.0, -1024.0, 1366.0, 1024.0), work: Rect::new(0.0, -1024.0, 1366.0, 1024.0) };
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0), up];
        let ws = [win(1, 0.0, 0.0, 1920.0, 1040.0)];
        let ps = platforms(&screens, &ws, PET, true);
        // 頭は上の画面に入る部分(0〜1366)だけ
        assert_eq!(of(&ps, 1), vec![(0.0, 0.0, 1366.0)]);
        // 上の画面の下端も床
        assert!(ps.iter().any(|p| p.owner == Owner::Floor { screen: 1 } && p.y == 0.0));
    }

    #[test]
    fn 横に並べた画面にまたがるウィンドウは_続いた1本の足場() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0), screen(1920.0, 0.0, 1920.0, 1080.0)];
        let ws = [win(1, 1500.0, 500.0, 900.0, 300.0)];
        let ps = platforms(&screens, &ws, PET, true);
        assert_eq!(of(&ps, 1), vec![(500.0, 1500.0, 2400.0)]);
    }

    #[test]
    fn 高さのずれた画面では_画面の無い所を足場から除く() {
        // 左に縦置きの画面(1080×1920、上に400ずれる)、右に横置きの主な画面
        let portrait = ScreenArea { full: Rect::new(-1080.0, -400.0, 1080.0, 1920.0), work: Rect::new(-1080.0, -400.0, 1080.0, 1880.0) };
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0), portrait];
        // 縦置きの画面の中ほど(y=1300)から主な画面の外(下)へ出るウィンドウ
        let ws = [win(1, -900.0, 1300.0, 1400.0, 100.0)];
        let ps = platforms(&screens, &ws, PET, true);
        // 主な画面の作業領域の下端は1040なので、y=1300で足もとが入るのは縦置きの画面だけ
        assert_eq!(of(&ps, 1), vec![(1300.0, -900.0, 0.0)]);
    }

    #[test]
    fn ウィンドウに乗らない設定なら床だけ() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0)];
        let ws = [win(1, 300.0, 500.0, 800.0, 300.0)];
        let ps = platforms(&screens, &ws, PET, false);
        assert_eq!(ps.len(), 1);
        assert!(matches!(ps[0].owner, Owner::Floor { .. }));
    }

    #[test]
    fn 前面が画面全体を覆えば全画面() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0), screen(1920.0, 0.0, 1920.0, 1080.0)];
        assert_eq!(fullscreen_screen(&screens, &[win(1, 1920.0, 0.0, 1920.0, 1080.0)]), Some(1));
        assert_eq!(fullscreen_screen(&screens, &[win(1, 0.0, 0.0, 1920.0, 1040.0)]), None);
        // 前面でない(focusedでない)ウィンドウは見ない
        assert_eq!(fullscreen_screen(&screens, &[win(2, 0.0, 0.0, 1920.0, 1080.0)]), None);
    }

    #[test]
    fn 点を含む画面() {
        let screens = [screen(0.0, 0.0, 1920.0, 1080.0), screen(1920.0, 0.0, 1920.0, 1080.0)];
        assert_eq!(screen_index_at(&screens, 1919.0, 10.0), Some(0));
        assert_eq!(screen_index_at(&screens, 1920.0, 10.0), Some(1));
        assert_eq!(screen_index_at(&screens, 10.0, 1080.0), None);
    }
}
