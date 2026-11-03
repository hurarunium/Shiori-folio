//! ペットの状態と動き(詩織Ver6.0の段階4。ペットの行動の設計書 `design-shiori-pet-behavior-20261103` の3節で広げた)。
//!
//! 位置と速さは座標の単位で持ち、時間とカーソル・ボタン・無操作の秒数は外から渡す(テストで決められるように)。
//! 押したのか、つまんだのかは、ボタンを押したまま少し動かしたかで分ける。
//!
//! いる場所(居場所)は、画面ごとの床か、ほかのアプリのウィンドウの上辺。全部の画面をつなげた1つの世界で動き、
//! 隣に画面が無い辺だけを壁と天井にする。乗っているウィンドウが動けば一緒に動き、閉じる・隠れると落ちる。

use super::surfaces::{self, Owner, Platform, Presence, ScreenArea};
use serde::Serialize;

/// 重力(ドット/秒²)。倍率を掛けて画素にする
const GRAVITY: f64 = 2400.0;
/// 歩く速さ(ドット/秒)
const WALK_SPEED: f64 = 40.0;
/// 押したまま、これより動かしたら「つまんだ」とみなす(ドット)
const DRAG_START: f64 = 4.0;
/// 投げたとみなす速さ(ドット/秒)
const THROW_SPEED: f64 = 120.0;
/// 床で普段のとき、画面の端からこの割合の中で過ごす
const EDGE_ZONE: f64 = 0.2;
/// 普段の行動を選ぶとき、飛び移るを選ぶ割合(Phase 3で点数に置き換える)
const JUMP_SHARE: f64 = 0.2;
/// 飛び移れる距離: 横はいる画面の幅の割合まで、上はペットの背丈の倍まで(下は制限なし。設計書の4節)
const JUMP_REACH_X: f64 = 0.4;
const JUMP_REACH_UP: f64 = 3.0;
/// 跳ぶ線が画面の中を通るかを確かめる点の数
const JUMP_SAMPLES: usize = 16;
/// 縁に座るとき、絵を下げる量(ドット)。足を垂らす絵(Phase 5)に合わせて決める。それまでは下げない
const SIT_DROP: f64 = 0.0;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Appear,
    Normal,
    Sleep,
    Wake,
    Notify,
    Happy,
    React,
    Dangle,
    Fly,
    Land,
    /// 足場から足場へ跳ぶ(飛び乗る・飛び降りる・飛び移る・画面を移る)
    Jump,
}

/// 普段のときに、していること
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Activity {
    Walk,
    Idle,
    Read,
    /// 縁に座る(ウィンドウの上だけ)
    Sit,
}

/// ペットのいる世界(座標の単位。Windowsは物理の画素、Macは論理の点)
#[derive(Clone, Debug)]
pub struct World {
    /// 1ドットが座標の単位でいくつか
    pub scale: f64,
    /// ペットの絵の幅と高さ
    pub size: (f64, f64),
    /// 全部の画面
    pub screens: Vec<ScreenArea>,
    /// ペットの真ん中がある画面(現れる床、画面が外れたときに戻る床)
    pub current: usize,
    /// 足場(画面ごとの床と、ウィンドウの上辺。`surfaces::platforms`)
    pub platforms: Vec<Platform>,
    /// 乗っているウィンドウの、いまのようす(毎回読む)
    pub ride: Option<(u64, Presence)>,
    /// 座らない上辺の幅(ウィンドウの左端から、右端から)。ボタンが付いている所
    pub caption: (f64, f64),
}

impl World {
    fn floor(&self, screen: usize) -> Option<&Platform> {
        self.platforms.iter().find(|p| p.owner == Owner::Floor { screen })
    }

    /// いる画面の床(無ければ、どれかの床)
    fn home_floor(&self) -> Option<&Platform> {
        self.floor(self.current).or_else(|| self.platforms.iter().find(|p| matches!(p.owner, Owner::Floor { .. })))
    }

    fn screen_at(&self, x: f64, y: f64) -> Option<usize> {
        surfaces::screen_index_at(&self.screens, x, y)
    }

    /// 絵の左上が(x, y)のとき、左右の端がどれかの画面の中か(隣の画面ならよい)
    fn fits_x(&self, x: f64, y: f64) -> bool {
        let mid = y + self.size.1 / 2.0;
        self.screen_at(x, mid).is_some() && self.screen_at(x + self.size.0 - 1.0, mid).is_some()
    }

    /// 絵の左上が(x, y)のとき、頭がどれかの画面の作業領域の中か
    fn fits_top(&self, x: f64, y: f64) -> bool {
        let cx = x + self.size.0 / 2.0;
        self.screens.iter().any(|s| s.work.contains(cx, y))
    }

    /// 真ん中が(cx, cy)にある点が、どれかの画面の中か
    fn inside(&self, cx: f64, cy: f64) -> bool {
        self.screen_at(cx, cy).is_some()
    }
}

/// いまの居場所。`dx` は、持ち主の左端(ウィンドウの左。床は0)からの、絵の左上のx
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    pub owner: Owner,
    pub dx: f64,
}

/// 跳ぶ計画(放物線)。時間を距離で決め、着く点を通るように初めの速さを決める
#[derive(Clone, Copy, Debug)]
struct Jump {
    from: (f64, f64),
    to: (f64, f64),
    total: f64,
    vx: f64,
    vy0: f64,
    g: f64,
    target: Owner,
    /// 着く足場の、計算したときの持ち主の左端
    anchor: f64,
}

impl Jump {
    fn new(from: (f64, f64), to: (f64, f64), scale: f64, target: Owner, anchor: f64) -> Self {
        let dist = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt() / scale;
        let total = (0.45 + dist / 700.0).clamp(0.45, 1.1);
        let g = GRAVITY * scale;
        let vx = (to.0 - from.0) / total;
        let vy0 = (to.1 - from.1 - 0.5 * g * total * total) / total;
        Self { from, to, total, vx, vy0, g, target, anchor }
    }

    fn at(&self, t: f64) -> (f64, f64) {
        (self.from.0 + self.vx * t, self.from.1 + self.vy0 * t + 0.5 * self.g * t * t)
    }

    fn velocity(&self, t: f64) -> (f64, f64) {
        (self.vx, self.vy0 + self.g * t)
    }

    /// 跳ぶ線の途中で、ペットの真ん中が画面の無い所に出ないか
    fn path_ok(&self, w: &World) -> bool {
        let (pw, ph) = w.size;
        (1..JUMP_SAMPLES).all(|i| {
            let (x, y) = self.at(self.total * i as f64 / JUMP_SAMPLES as f64);
            w.inside(x + pw / 2.0, y + ph / 2.0)
        })
    }

    /// 行き先の足場が、まだ同じ所にあるか
    fn target_ok(&self, w: &World) -> bool {
        let (pw, ph) = w.size;
        let (cx, feet) = (self.to.0 + pw / 2.0, self.to.1 + ph);
        w.platforms.iter().any(|p| p.owner == self.target && (p.y - feet).abs() < 2.0 && cx >= p.x0 && cx <= p.x1)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Input {
    pub cursor: Option<(f64, f64)>,
    pub button_down: bool,
    /// 最後の入力からの秒数(取れなければNone。眠る動きを止める)
    pub idle_seconds: Option<f64>,
    pub sleep_after: f64,
}

/// 1回の計算で起きたこと(本体が、本のメニューを開くなどに使う)
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Pressed,
    StateChanged(State),
}

pub struct Brain {
    pub x: f64,
    pub y: f64,
    vx: f64,
    vy: f64,
    /// 1: 右向き、-1: 左向き
    pub facing: f64,
    pub state: State,
    pub activity: Activity,
    /// いまの状態になってからの秒数(絵のコマはこれで決める)
    pub state_t: f64,
    /// いまの状態を終える秒数(Noneなら終わりを決めない)
    until: Option<f64>,
    /// 普段のときに、次の過ごし方を選ぶまでの秒数
    activity_left: f64,
    target_x: Option<f64>,
    /// 押した点(つまむ前)。押したまま動かしたら、つまむ
    press: Option<(f64, f64)>,
    grab: Option<(f64, f64)>,
    drag_prev: Option<(f64, f64)>,
    rng: u64,
    /// いまの居場所(つままれている・飛んでいる・跳んでいる間はNone)
    pub stand: Option<Stand>,
    /// いまの居場所の持ち主の左端(ウィンドウなら毎回読み直す。床は0)
    anchor_now: f64,
    /// ウィンドウの上にいるときの、その上辺の高さ(それより下の点はクリックを通す)
    pub edge_y: Option<f64>,
    jump: Option<Jump>,
}

impl Brain {
    /// いる画面の床の右端に現れる
    pub fn new(w: &World, seed: u64) -> Self {
        let (pw, ph) = w.size;
        let (x, y, stand) = match w.home_floor() {
            Some(p) => {
                let x = (p.x1 - pw - 20.0 * w.scale).clamp(p.x0, (p.x1 - pw).max(p.x0));
                (x, p.y - ph, Some(Stand { owner: p.owner, dx: x }))
            }
            None => (0.0, 0.0, None),
        };
        Self {
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            facing: -1.0,
            state: State::Appear,
            activity: Activity::Idle,
            state_t: 0.0,
            until: Some(1.0),
            activity_left: 0.0,
            target_x: None,
            press: None,
            grab: None,
            drag_prev: None,
            rng: seed | 1,
            stand,
            anchor_now: 0.0,
            edge_y: None,
            jump: None,
        }
    }

    fn rand(&mut self) -> f64 {
        self.rng = self.rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.rng >> 33) as f64 / (1u64 << 31) as f64
    }

    fn set(&mut self, state: State, until: Option<f64>, events: &mut Vec<Event>) {
        if self.state != state {
            events.push(Event::StateChanged(state));
        }
        self.state = state;
        self.state_t = 0.0;
        self.until = until;
    }

    /// 乗っているウィンドウ(本体が、そのウィンドウのようすを毎回読むのに使う)
    pub fn riding(&self) -> Option<u64> {
        match self.stand?.owner {
            Owner::Window { id } => Some(id),
            Owner::Floor { .. } => None,
        }
    }

    fn on_window(&self) -> bool {
        self.riding().is_some()
    }

    /// 普段に戻る。`to_edge` なら(床にいるときだけ)、近いほうの端へ歩いていく
    fn to_normal(&mut self, w: &World, to_edge: bool, events: &mut Vec<Event>) {
        self.set(State::Normal, None, events);
        let range = self.range(w);
        if to_edge && !self.on_window() {
            if let Some((lo, hi)) = range {
                let edge = if self.x < (lo + hi) / 2.0 { lo + 16.0 * w.scale } else { hi - 16.0 * w.scale };
                self.walk_to(edge.clamp(lo, hi));
                return;
            }
        }
        self.activity = Activity::Idle;
        self.activity_left = 3.0;
    }

    fn walk_to(&mut self, x: f64) {
        self.target_x = Some(x);
        self.activity = Activity::Walk;
        self.facing = if x < self.x { -1.0 } else { 1.0 };
    }

    /// いまの足場の上で、絵の左上のxが取れる範囲
    fn range(&self, w: &World) -> Option<(f64, f64)> {
        let st = self.stand?;
        let pw = w.size.0;
        let p = match st.owner {
            Owner::Floor { screen } => *w.floor(screen)?,
            Owner::Window { .. } => {
                // ウィンドウからの距離で、いまいる見えている部分を探す
                let cx = st.dx + pw / 2.0;
                let p = w.platforms.iter().find(|p| p.owner == st.owner && cx >= p.x0 - p.anchor && cx <= p.x1 - p.anchor)?;
                let shift = self.anchor_now - p.anchor;
                Platform { x0: p.x0 + shift, x1: p.x1 + shift, ..*p }
            }
        };
        Some((p.x0, (p.x1 - pw).max(p.x0)))
    }

    /// xを動かし、居場所の中の位置も合わせる
    fn move_x(&mut self, x: f64) {
        self.x = x;
        if let Some(st) = self.stand.as_mut() {
            st.dx = x - self.anchor_now;
        }
    }

    /// 普段のときの次の過ごし方: 飛び移る・本を読む・待つ(ウィンドウの上では座ることも)・少し歩く
    fn choose_activity(&mut self, w: &World, events: &mut Vec<Event>) {
        if self.rand() < JUMP_SHARE {
            let plans = self.jump_candidates(w);
            if !plans.is_empty() {
                let i = ((self.rand() * plans.len() as f64) as usize).min(plans.len() - 1);
                self.start_jump(plans[i], events);
                return;
            }
        }
        let Some((lo, hi)) = self.range(w) else { return };
        let r = self.rand();
        if r < 0.4 {
            self.activity = Activity::Read;
            self.activity_left = 10.0 + self.rand() * 15.0;
        } else if r < 0.75 {
            self.activity = if self.on_window() && self.can_sit(w) && self.rand() < 0.5 { Activity::Sit } else { Activity::Idle };
            self.activity_left = 4.0 + self.rand() * 6.0;
        } else if self.on_window() {
            // ウィンドウの上では、上辺の見えている部分のどこへでも
            let x = lo + self.rand() * (hi - lo);
            self.walk_to(x);
        } else {
            // 床では、端の近くにとどまる(端から EDGE_ZONE の中)
            let zone = (hi - lo) * EDGE_ZONE;
            let (zlo, zhi) = if self.x < (lo + hi) / 2.0 { (lo, lo + zone) } else { (hi - zone, hi) };
            let step = (30.0 + self.rand() * 90.0) * w.scale * if self.rand() < 0.5 { -1.0 } else { 1.0 };
            self.walk_to((self.x + step).clamp(zlo, zhi));
        }
    }

    /// 上辺のボタンが付いている所では座らない(足を垂らすと、ボタンにかぶるため)
    fn can_sit(&self, w: &World) -> bool {
        let Some((_, Presence::Visible(r))) = w.ride else { return false };
        let (left, right) = (self.x, self.x + w.size.0);
        left >= r.x + w.caption.0 && right <= r.right() - w.caption.1
    }

    /// 飛び移れる足場ごとの、跳ぶ計画
    fn jump_candidates(&mut self, w: &World) -> Vec<Jump> {
        let Some(st) = self.stand else { return Vec::new() };
        let (pw, ph) = w.size;
        let reach_x = w.screens.get(w.current).map_or(1000.0, |s| s.work.w * JUMP_REACH_X);
        let mut out = Vec::new();
        for p in w.platforms.clone() {
            if p.owner == st.owner || p.x1 - p.x0 < pw {
                continue;
            }
            // いまのxに近い所へ、少しずらして着く
            let near = self.x.clamp(p.x0, p.x1 - pw);
            let tx = (near + (self.rand() - 0.5) * pw).clamp(p.x0, p.x1 - pw);
            let ty = p.y - ph;
            if (tx - self.x).abs() > reach_x || self.y - ty > JUMP_REACH_UP * ph {
                continue;
            }
            let plan = Jump::new((self.x, self.y), (tx, ty), w.scale, p.owner, p.anchor);
            if plan.path_ok(w) {
                out.push(plan);
            }
        }
        out
    }

    fn start_jump(&mut self, plan: Jump, events: &mut Vec<Event>) {
        self.stand = None;
        self.edge_y = None;
        self.target_x = None;
        self.facing = if plan.to.0 < plan.from.0 { -1.0 } else { 1.0 };
        self.jump = Some(plan);
        self.set(State::Jump, None, events);
    }

    /// 居場所から外れて落ちる
    fn start_fly(&mut self, events: &mut Vec<Event>) {
        self.stand = None;
        self.edge_y = None;
        self.jump = None;
        self.target_x = None;
        self.set(State::Fly, None, events);
    }

    /// 足場に着地する
    fn land_on(&mut self, p: &Platform, w: &World, events: &mut Vec<Event>) {
        let (pw, ph) = w.size;
        self.y = p.y - ph;
        self.x = self.x.clamp(p.x0, (p.x1 - pw).max(p.x0));
        self.vx = 0.0;
        self.vy = 0.0;
        self.jump = None;
        self.anchor_now = p.anchor;
        self.stand = Some(Stand { owner: p.owner, dx: self.x - p.anchor });
        self.edge_y = self.on_window().then_some(p.y);
        self.set(State::Land, Some(0.5), events);
    }

    /// いる画面の床に移って着地する(乗っているウィンドウが別の仮想デスクトップへ移った、画面が外れた。
    /// 切り替えのたびに落ちないよう、落ちる動きにしない。ふらる、11/3)
    fn land_home(&mut self, w: &World, events: &mut Vec<Event>) {
        match w.home_floor().copied() {
            Some(p) => self.land_on(&p, w, events),
            None => self.start_fly(events),
        }
    }

    /// 居場所について行く。乗っているウィンドウが動けば一緒に動き、無くなれば落ちる。居場所に立っていればtrue
    fn follow(&mut self, w: &World, events: &mut Vec<Event>) -> bool {
        let (pw, ph) = w.size;
        let Some(st) = self.stand else {
            self.start_fly(events);
            return false;
        };
        match st.owner {
            Owner::Floor { screen } => match w.floor(screen).copied() {
                Some(p) => {
                    self.anchor_now = 0.0;
                    self.edge_y = None;
                    self.y = p.y - ph;
                    self.move_x(self.x.clamp(p.x0, (p.x1 - pw).max(p.x0)));
                    true
                }
                None => {
                    self.land_home(w, events);
                    false
                }
            },
            Owner::Window { id } => match w.ride {
                Some((rid, Presence::Visible(r))) if rid == id => {
                    self.anchor_now = r.x;
                    self.x = r.x + st.dx;
                    let drop = if self.state == State::Normal && self.activity == Activity::Sit { SIT_DROP * w.scale } else { 0.0 };
                    self.y = r.y - ph + drop;
                    self.edge_y = Some(r.y);
                    // まだ足場か(前に別のウィンドウが来て隠れていない、頭が画面からはみ出ていない)
                    let cx = st.dx + pw / 2.0;
                    let ok = w.platforms.iter().any(|p| p.owner == st.owner && cx >= p.x0 - p.anchor && cx <= p.x1 - p.anchor);
                    if !ok {
                        self.start_fly(events);
                    }
                    ok
                }
                Some((rid, Presence::Hidden)) if rid == id => {
                    self.land_home(w, events);
                    false
                }
                _ => {
                    self.start_fly(events);
                    false
                }
            },
        }
    }

    /// 落ちる途中で、足場の線を上から横切ったか。どれかの画面の床より下に出たら、その床
    fn landing(&self, w: &World, prev_feet: f64) -> Option<Platform> {
        let (pw, ph) = w.size;
        let (cx, feet) = (self.x + pw / 2.0, self.y + ph);
        let crossed = w
            .platforms
            .iter()
            .filter(|p| p.y >= prev_feet - 0.5 && p.y <= feet && cx >= p.x0 && cx <= p.x1)
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .copied();
        if crossed.is_some() {
            return crossed;
        }
        // いる画面の床より下(画面の下の帯など)なら、その床に戻す
        if let Some(s) = w.screen_at(cx, self.y + ph / 2.0) {
            if let Some(p) = w.floor(s) {
                if feet >= p.y {
                    return Some(*p);
                }
            }
            return None;
        }
        // どの画面からも出たら、真下か真上の床(xが入る床のうち、足もとより上で一番低いもの)
        w.platforms
            .iter()
            .filter(|p| matches!(p.owner, Owner::Floor { .. }) && cx >= p.x0 && cx <= p.x1 && p.y <= feet)
            .max_by(|a, b| a.y.total_cmp(&b.y))
            .or_else(|| w.home_floor())
            .copied()
    }

    /// 押された(画面のmousedown)。つまむか、押しただけかは、このあとの動きで決める
    pub fn press(&mut self, cursor: (f64, f64)) {
        if matches!(self.state, State::Dangle | State::Fly | State::Jump) {
            return;
        }
        self.press = Some(cursor);
    }

    /// 知らせ。Claudeの作業の終わりは喜ぶ。眠っていても起きて知らせる
    pub fn notice(&mut self, happy: bool) -> Vec<Event> {
        let mut events = Vec::new();
        if matches!(self.state, State::Dangle | State::Fly | State::Jump) {
            return events;
        }
        if happy {
            self.set(State::Happy, Some(2.0), &mut events);
        } else {
            self.set(State::Notify, Some(5.0), &mut events);
        }
        events
    }

    pub fn step(&mut self, dt: f64, w: &World, input: &Input) -> Vec<Event> {
        let mut events = Vec::new();
        self.state_t += dt;
        let s = w.scale;
        let (pw, ph) = w.size;

        // 押したまま動かしたら、つまむ。動かさずに離したら、押しただけ
        if let Some((px, py)) = self.press {
            if !input.button_down {
                self.press = None;
                events.push(Event::Pressed);
                if !matches!(self.state, State::Notify | State::Happy) {
                    self.set(State::React, Some(0.4), &mut events);
                }
            } else if let Some((cx, cy)) = input.cursor {
                if ((cx - px).powi(2) + (cy - py).powi(2)).sqrt() > DRAG_START * s {
                    self.press = None;
                    self.grab = Some((px - self.x, py - self.y));
                    self.drag_prev = None;
                    self.vx = 0.0;
                    self.vy = 0.0;
                    self.stand = None;
                    self.edge_y = None;
                    self.jump = None;
                    self.set(State::Dangle, None, &mut events);
                }
            }
        }

        match self.state {
            State::Dangle => {
                if !input.button_down {
                    self.grab = None;
                    let fast = (self.vx.powi(2) + self.vy.powi(2)).sqrt() > THROW_SPEED * s;
                    // ゆっくり離して、足もとがちょうど足場の上なら、そこに着地する。ほかは落ちる
                    let (cx, feet) = (self.x + pw / 2.0, self.y + ph);
                    let here = w.platforms.iter().find(|p| (p.y - feet).abs() <= 1.0 && cx >= p.x0 && cx <= p.x1).copied();
                    match here {
                        Some(p) if !fast => self.land_on(&p, w, &mut events),
                        _ => {
                            let below_floor = self.landing(w, feet).filter(|p| feet > p.y + 1.0);
                            match below_floor {
                                // 床より下で離した(画面の下の帯など)ときは、その床に戻す
                                Some(p) if !fast => self.land_on(&p, w, &mut events),
                                _ => self.set(State::Fly, None, &mut events),
                            }
                        }
                    }
                } else if let (Some((gx, gy)), Some((cx, cy))) = (self.grab, input.cursor) {
                    let (nx, ny) = (cx - gx, cy - gy);
                    if let Some((px, py)) = self.drag_prev {
                        if dt > 0.0 {
                            // ならした動きを、投げたときの勢いにする
                            self.vx = self.vx * 0.6 + (nx - px) / dt * 0.4;
                            self.vy = self.vy * 0.6 + (ny - py) / dt * 0.4;
                        }
                    }
                    self.drag_prev = Some((nx, ny));
                    self.x = nx;
                    self.y = ny;
                }
            }
            State::Fly => {
                self.vy += GRAVITY * s * dt;
                let (ox, oy) = (self.x, self.y);
                let (mut nx, mut ny) = (ox + self.vx * dt, oy + self.vy * dt);
                // 隣に画面が無い辺は、壁と天井(跳ね返る・止まる)
                if !w.fits_x(nx, ny) && w.fits_x(ox, ny) {
                    nx = ox;
                    self.vx = -self.vx * 0.5;
                }
                if self.vy < 0.0 && !w.fits_top(nx, ny) {
                    ny = oy;
                    self.vy = 0.0;
                }
                self.x = nx;
                self.y = ny;
                if self.vx.abs() > 1.0 {
                    self.facing = self.vx.signum();
                }
                if self.vy >= 0.0 {
                    if let Some(p) = self.landing(w, oy + ph) {
                        self.land_on(&p, w, &mut events);
                    }
                }
            }
            State::Jump => {
                if let Some(plan) = self.jump {
                    let t = self.state_t.min(plan.total);
                    let (x, y) = plan.at(t);
                    self.x = x;
                    self.y = y;
                    if input.button_down || !plan.target_ok(w) {
                        // ボタンが押された(ふらるがウィンドウを操作し始めた)、行き先が消えた・動いたら、そのまま落ちる
                        let (vx, vy) = plan.velocity(t);
                        self.start_fly(&mut events);
                        self.vx = vx;
                        self.vy = vy;
                    } else if self.state_t >= plan.total {
                        // 同じウィンドウの上辺が隠れて分かれていることがあるので、着く点を含む部分を選ぶ
                        let cx = plan.to.0 + pw / 2.0;
                        let target =
                            w.platforms.iter().find(|p| p.owner == plan.target && cx >= p.x0 && cx <= p.x1).copied();
                        self.x = plan.to.0;
                        match target {
                            Some(p) => self.land_on(&Platform { anchor: plan.anchor, ..p }, w, &mut events),
                            None => self.start_fly(&mut events),
                        }
                    }
                }
            }
            State::Normal => {
                if self.follow(w, &mut events) {
                    let sleepy = input.idle_seconds.is_some_and(|i| i >= input.sleep_after);
                    if sleepy {
                        self.target_x = None;
                        self.set(State::Sleep, None, &mut events);
                    } else if self.activity == Activity::Walk {
                        let (lo, hi) = self.range(w).unwrap_or((self.x, self.x));
                        let target = self.target_x.unwrap_or(self.x).clamp(lo, hi);
                        let step = WALK_SPEED * s * dt;
                        if (target - self.x).abs() <= step {
                            self.move_x(target);
                            self.target_x = None;
                            self.choose_activity(w, &mut events);
                        } else {
                            self.facing = (target - self.x).signum();
                            self.move_x(self.x + self.facing * step);
                        }
                    } else {
                        self.activity_left -= dt;
                        if self.activity_left <= 0.0 {
                            self.choose_activity(w, &mut events);
                        }
                    }
                }
            }
            State::Sleep => {
                if self.follow(w, &mut events) && input.idle_seconds.is_some_and(|i| i < 1.0) {
                    self.set(State::Wake, Some(1.25), &mut events);
                }
            }
            _ => {
                // 現れる・起きる・知らせ・喜ぶ・反応する・着地は、その場で決まった時間だけ(居場所が動けば一緒に動く)
                self.follow(w, &mut events);
            }
        }

        if let Some(until) = self.until {
            if self.state_t >= until {
                match self.state {
                    State::Appear | State::Land => self.to_normal(w, true, &mut events),
                    _ => self.to_normal(w, false, &mut events),
                }
            }
        }
        events
    }

    /// いまの絵の動きの名前(`sprites.json` の名前。絵が無い動きは `sprites.rs` が近い絵に替える)
    pub fn anim(&self) -> &'static str {
        match self.state {
            State::Appear => "appear",
            State::Normal => match (self.activity, self.on_window()) {
                (Activity::Walk, _) => "walk",
                (Activity::Read, _) => "read",
                (Activity::Idle, false) => "idle",
                (Activity::Idle, true) => "perch_stand",
                (Activity::Sit, _) => "perch_sit",
            },
            State::Sleep => "sleep",
            State::Wake => "wake",
            State::Notify => "notify",
            State::Happy => "happy",
            State::React => "react",
            State::Dangle => "dangle",
            State::Fly => {
                if self.vy < 0.0 {
                    "fly"
                } else {
                    "fall"
                }
            }
            State::Land => "land",
            State::Jump => match self.jump {
                Some(j) if j.to.1 < j.from.1 => "jump_on",
                _ => "jump_off",
            },
        }
    }

    pub fn grabbing(&self) -> bool {
        self.press.is_some() || self.grab.is_some()
    }

    /// 跳んでいる・飛んでいる・つままれている(この間は、画面をまたいでも大きさを変えない)
    pub fn airborne(&self) -> bool {
        matches!(self.state, State::Jump | State::Fly | State::Dangle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pet::surfaces::Rect;

    const PET: (f64, f64) = (128.0, 192.0);

    /// 横置きの画面(下に40の帯)
    fn screen(x: f64, y: f64, w: f64, h: f64) -> ScreenArea {
        ScreenArea { full: Rect::new(x, y, w, h), work: Rect::new(x, y, w, h - 40.0) }
    }

    /// 画面だけの世界(足場は床)
    fn world_of(screens: Vec<ScreenArea>) -> World {
        let platforms = surfaces::platforms(&screens, &[], PET, true);
        World { scale: 1.0, size: PET, screens, current: 0, platforms, ride: None, caption: (0.0, 140.0) }
    }

    /// 今までのテストの世界: 床の上で、絵の左上のxが0〜1792、yが840
    fn world() -> World {
        world_of(vec![screen(0.0, 0.0, 1920.0, 1072.0)])
    }

    fn win(id: u64, x: f64, y: f64, w: f64, h: f64) -> surfaces::WinInfo {
        surfaces::WinInfo { id, app: format!("app{id}"), rect: Rect::new(x, y, w, h), focused: false }
    }

    /// ウィンドウのある世界。乗っているウィンドウのようすも、一覧の位置のまま見せる
    fn with_windows(mut w: World, wins: &[surfaces::WinInfo], riding: Option<u64>) -> World {
        w.platforms = surfaces::platforms(&w.screens, wins, PET, true);
        w.ride = riding.and_then(|id| wins.iter().find(|x| x.id == id)).map(|x| (x.id, Presence::Visible(x.rect)));
        w
    }

    fn run_in(b: &mut Brain, w: &World, secs: f64, input: &Input) -> Vec<Event> {
        let mut all = Vec::new();
        let n = (secs / 0.016).ceil() as usize;
        for _ in 0..n {
            all.extend(b.step(0.016, w, input));
        }
        all
    }

    fn run(b: &mut Brain, secs: f64, input: &Input) -> Vec<Event> {
        run_in(b, &world(), secs, input)
    }

    fn awake() -> Input {
        Input { idle_seconds: Some(0.5), sleep_after: 300.0, ..Default::default() }
    }

    /// 窓の上に乗った状態にする(真上から落とす)
    fn drop_onto(b: &mut Brain, w: &World, x: f64) {
        b.stand = None;
        b.x = x;
        b.y = 0.0;
        b.state = State::Fly;
        run_in(b, w, 1.5, &awake());
    }

    /// ウィンドウの上に直接置く(落とすと先に別の足場に着くとき)
    fn place_on(b: &mut Brain, w: &World, id: u64, rect: Rect, x: f64) {
        b.stand = Some(Stand { owner: Owner::Window { id }, dx: x - rect.x });
        b.state = State::Normal;
        b.activity = Activity::Idle;
        b.activity_left = 100.0;
        b.step(0.016, w, &awake());
    }

    #[test]
    fn 現れてから普段になり_端に寄る() {
        let mut b = Brain::new(&world(), 7);
        assert_eq!(b.state, State::Appear);
        assert!((b.y - 840.0).abs() < 1e-9);
        run(&mut b, 1.1, &awake());
        assert_eq!(b.state, State::Normal);
        run(&mut b, 3.0, &awake());
        // 右の端(右から16ドット)に寄っている
        assert!(b.x > 1792.0 - 1792.0 * EDGE_ZONE - 1.0);
    }

    #[test]
    fn 触らないと眠り_触ると起きる() {
        let mut b = Brain::new(&world(), 7);
        run(&mut b, 1.1, &awake());
        let sleepy = Input { idle_seconds: Some(301.0), sleep_after: 300.0, ..Default::default() };
        run(&mut b, 0.1, &sleepy);
        assert_eq!(b.state, State::Sleep);
        run(&mut b, 0.1, &awake());
        assert_eq!(b.state, State::Wake);
        run(&mut b, 1.3, &awake());
        assert_eq!(b.state, State::Normal);
    }

    #[test]
    fn 押しただけなら反応し_動かしたらつまみ_離すと落ちて着地する() {
        let mut b = Brain::new(&world(), 7);
        run(&mut b, 1.1, &awake());
        let at = (b.x + 60.0, b.y + 100.0);
        b.press(at);
        let mut down = awake();
        down.button_down = true;
        down.cursor = Some(at);
        run(&mut b, 0.05, &down);
        let ev = run(&mut b, 0.02, &awake());
        assert!(ev.contains(&Event::Pressed));
        assert_eq!(b.state, State::React);

        // つまんで上へ持ち上げ、離す
        run(&mut b, 0.5, &awake());
        b.press(at);
        down.cursor = Some((at.0, at.1 - 300.0));
        run(&mut b, 0.1, &down);
        assert_eq!(b.state, State::Dangle);
        assert!(b.y < 840.0 - 200.0);
        run(&mut b, 0.02, &awake());
        assert_eq!(b.state, State::Fly);
        run(&mut b, 2.0, &awake());
        assert!((b.y - 840.0).abs() < 1e-9);
        assert!(matches!(b.state, State::Land | State::Normal));
    }

    #[test]
    fn 知らせはその場で_Claudeは喜ぶ() {
        let mut b = Brain::new(&world(), 7);
        run(&mut b, 1.1, &awake());
        let x = b.x;
        b.notice(false);
        assert_eq!(b.state, State::Notify);
        assert_eq!(b.anim(), "notify");
        run(&mut b, 1.0, &awake());
        assert!((b.x - x).abs() < 1e-9);
        b.notice(true);
        assert_eq!(b.state, State::Happy);
        run(&mut b, 2.1, &awake());
        assert_eq!(b.state, State::Normal);
    }

    #[test]
    fn 落ちる途中でウィンドウの上辺に着地し_動かすと一緒に動き_閉じると落ちる() {
        let wins = [win(5, 400.0, 500.0, 800.0, 300.0)];
        let w = with_windows(world(), &wins, Some(5));
        let mut b = Brain::new(&w, 7);
        drop_onto(&mut b, &w, 600.0);
        assert_eq!(b.riding(), Some(5));
        assert!((b.y - (500.0 - 192.0)).abs() < 1e-9);
        assert_eq!(b.edge_y, Some(500.0));

        // ウィンドウを右下へ動かすと、一緒に動く(足場の一覧はまだ古いまま)
        let mut moved = w.clone();
        moved.ride = Some((5, Presence::Visible(Rect::new(500.0, 560.0, 800.0, 300.0))));
        let x = b.x;
        b.step(0.016, &moved, &awake());
        assert!((b.x - (x + 100.0)).abs() < 1e-9);
        assert!((b.y - (560.0 - 192.0)).abs() < 1e-9);

        // 閉じると落ちて、床に着く
        let mut gone = world();
        gone.ride = Some((5, Presence::Gone));
        b.step(0.016, &gone, &awake());
        assert_eq!(b.state, State::Fly);
        run_in(&mut b, &gone, 2.0, &awake());
        assert!((b.y - 840.0).abs() < 1e-9);
        assert_eq!(b.riding(), None);
    }

    #[test]
    fn 前に別のウィンドウが来て隠れると落ちる() {
        let wins = [win(5, 400.0, 500.0, 800.0, 300.0)];
        let w = with_windows(world(), &wins, Some(5));
        let mut b = Brain::new(&w, 7);
        drop_onto(&mut b, &w, 600.0);
        assert_eq!(b.riding(), Some(5));
        // 前に来たウィンドウ(6)が、上辺を全部覆う
        let covered = with_windows(world(), &[win(6, 300.0, 400.0, 1000.0, 400.0), wins[0].clone()], Some(5));
        b.step(0.016, &covered, &awake());
        assert_eq!(b.state, State::Fly);
    }

    #[test]
    fn 別の仮想デスクトップへ移ったら_落ちずに床に移る() {
        let wins = [win(5, 400.0, 500.0, 800.0, 300.0)];
        let w = with_windows(world(), &wins, Some(5));
        let mut b = Brain::new(&w, 7);
        drop_onto(&mut b, &w, 600.0);
        let mut hidden = world();
        hidden.ride = Some((5, Presence::Hidden));
        b.step(0.016, &hidden, &awake());
        assert_eq!(b.state, State::Land);
        assert!((b.y - 840.0).abs() < 1e-9);
        assert!(matches!(b.stand.unwrap().owner, Owner::Floor { .. }));
    }

    #[test]
    fn 床からウィンドウへ飛び移る() {
        let wins = [win(5, 1200.0, 600.0, 600.0, 300.0)];
        let mut w = with_windows(world(), &wins, None);
        let mut b = Brain::new(&w, 11);
        let mut landed = false;
        for _ in 0..4000 {
            b.step(0.016, &w, &awake());
            // 乗ったら、そのウィンドウのようすを渡す(本体と同じ)
            w.ride = b.riding().map(|id| (id, Presence::Visible(wins[0].rect)));
            if b.riding() == Some(5) {
                landed = true;
                break;
            }
        }
        assert!(landed, "床から近いウィンドウへ飛び移らなかった");
        assert!((b.y - (600.0 - 192.0)).abs() < 1e-9);
    }

    #[test]
    fn 跳ぶ途中でボタンが押されたら_そのまま落ちる() {
        let wins = [win(5, 1200.0, 600.0, 600.0, 300.0)];
        let w = with_windows(world(), &wins, None);
        let mut b = Brain::new(&w, 7);
        run_in(&mut b, &w, 1.1, &awake());
        let plans = b.jump_candidates(&w);
        assert!(!plans.is_empty());
        let mut ev = Vec::new();
        b.start_jump(plans[0], &mut ev);
        run_in(&mut b, &w, 0.1, &awake());
        let down = Input { button_down: true, ..awake() };
        b.step(0.016, &w, &down);
        assert_eq!(b.state, State::Fly);
    }

    #[test]
    fn 縦に並べた画面では_高いウィンドウから上の画面の床へ行ける() {
        // 下に主な画面、上にiPad(下端がy=0でつながる)
        let up = ScreenArea { full: Rect::new(0.0, -834.0, 1194.0, 834.0), work: Rect::new(0.0, -804.0, 1194.0, 804.0) };
        let w = world_of(vec![screen(0.0, 0.0, 1920.0, 1080.0), up]);
        // 主な画面の上端近くのウィンドウ(頭は上の画面に入る)
        let wins = [win(5, 100.0, 60.0, 800.0, 500.0)];
        let w = with_windows(w, &wins, Some(5));
        let mut b = Brain::new(&w, 7);
        place_on(&mut b, &w, 5, wins[0].rect, 300.0);
        assert_eq!(b.riding(), Some(5));
        assert!((b.y - (60.0 - 192.0)).abs() < 1e-9);
        let targets: Vec<Owner> = b.jump_candidates(&w).iter().map(|j| j.target).collect();
        assert!(targets.contains(&Owner::Floor { screen: 1 }), "上の画面の床へ跳べない: {targets:?}");
        // 下の画面の床からは、上の画面の床へは届かない(背丈の3倍より高い)
        let mut floor = Brain::new(&world_of(w.screens.clone()), 7);
        floor.x = 300.0;
        let targets: Vec<Owner> = floor.jump_candidates(&world_of(w.screens.clone())).iter().map(|j| j.target).collect();
        assert!(!targets.contains(&Owner::Floor { screen: 1 }));
    }

    #[test]
    fn 上の画面の床は床で_落ちても下の画面へは抜けない() {
        let up = ScreenArea { full: Rect::new(0.0, -834.0, 1194.0, 834.0), work: Rect::new(0.0, -804.0, 1194.0, 804.0) };
        let w = world_of(vec![screen(0.0, 0.0, 1920.0, 1080.0), up]);
        let mut b = Brain::new(&w, 7);
        b.stand = None;
        b.x = 300.0;
        b.y = -700.0;
        b.state = State::Fly;
        run_in(&mut b, &w, 1.0, &awake());
        assert_eq!(b.stand.map(|s| s.owner), Some(Owner::Floor { screen: 1 }));
        assert!((b.y - (0.0 - 192.0)).abs() < 1e-9);
    }

    #[test]
    fn 画面の無い所を通る飛び移りは選ばない() {
        // 左の画面の床(y=1040)から、右に高く離れた画面(上に600ずれ、間に隙間)の床へ
        let right = screen(2000.0, -600.0, 1920.0, 1080.0);
        let w = world_of(vec![screen(0.0, 0.0, 1920.0, 1080.0), right]);
        let mut b = Brain::new(&w, 7);
        b.x = 1700.0;
        let targets: Vec<Owner> = b.jump_candidates(&w).iter().map(|j| j.target).collect();
        assert!(!targets.contains(&Owner::Floor { screen: 1 }));
    }

    #[test]
    fn 横に並べた画面では_投げると隣の画面へ越え_端の画面では跳ね返る() {
        let two = world_of(vec![screen(0.0, 0.0, 1920.0, 1080.0), screen(1920.0, 0.0, 1920.0, 1080.0)]);
        let mut b = Brain::new(&two, 7);
        b.stand = None;
        b.x = 1700.0;
        b.y = 600.0;
        b.vx = 1500.0;
        b.vy = -300.0;
        b.state = State::Fly;
        run_in(&mut b, &two, 2.0, &awake());
        assert_eq!(b.stand.map(|s| s.owner), Some(Owner::Floor { screen: 1 }));

        let mut b = Brain::new(&world(), 7);
        b.stand = None;
        b.x = 1700.0;
        b.y = 600.0;
        b.vx = 1500.0;
        b.state = State::Fly;
        run(&mut b, 2.0, &awake());
        assert!(b.x <= 1792.0 + 1e-9);
        assert_eq!(b.stand.map(|s| s.owner), Some(Owner::Floor { screen: 0 }));
    }

    #[test]
    fn ボタンのある所では座らない() {
        let wins = [win(5, 400.0, 500.0, 800.0, 300.0)];
        let w = with_windows(world(), &wins, Some(5));
        let mut b = Brain::new(&w, 7);
        drop_onto(&mut b, &w, 500.0);
        assert!(b.can_sit(&w));
        // 右端から140はボタン(Windows)
        b.move_x(1200.0 - 128.0 - 50.0);
        assert!(!b.can_sit(&w));
    }
}
