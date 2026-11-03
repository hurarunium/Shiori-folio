//! ペットの行動の設定(ペットの行動の設計書 `design-shiori-pet-behavior-20261103` の4・6節)。
//!
//! 行動の重み・活発さ・アプリの好み・邪魔しない条件を `data/pet/behavior.json` に持つ。行動アルゴリズムの形は
//! 固定し、ここではパラメーターだけを変える(企画書のP-10)。ファイルが無い・読めないときは「普通」のひな形で動く。
//! tauriにもOSにも依存させない(Windowsでもこのファイルだけを小さなクレートでテストできるように)。

use serde::{Deserialize, Serialize};
use std::path::Path;

/// 行動(重みを持つもの。アプリの好きな行動にも使う)
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Stand,
    Sit,
    Read,
    Walk,
    Jump,
    Screen,
    Hang,
    Climb,
    Sleep,
}

/// 行動ごとのしやすさ(0〜10。0ならその行動をしない)
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Weights {
    pub stand: f64,
    pub sit: f64,
    pub read: f64,
    pub walk: f64,
    pub jump: f64,
    /// 画面を移る
    pub screen: f64,
    pub hang: f64,
    pub climb: f64,
    /// うたた寝(普段の途中で自分から眠る)。最後の入力から○分で眠るのは、これに関係なく残す
    pub sleep: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Preset::Normal.behavior().weights
    }
}

impl Weights {
    pub fn of(&self, a: Action) -> f64 {
        match a {
            Action::Stand => self.stand,
            Action::Sit => self.sit,
            Action::Read => self.read,
            Action::Walk => self.walk,
            Action::Jump => self.jump,
            Action::Screen => self.screen,
            Action::Hang => self.hang,
            Action::Climb => self.climb,
            Action::Sleep => self.sleep,
        }
    }

    fn clamped(mut self) -> Self {
        for v in [
            &mut self.stand,
            &mut self.sit,
            &mut self.read,
            &mut self.walk,
            &mut self.jump,
            &mut self.screen,
            &mut self.hang,
            &mut self.climb,
            &mut self.sleep,
        ] {
            *v = if v.is_finite() { v.clamp(0.0, 10.0) } else { 0.0 };
        }
        self
    }
}

/// アプリの好き度
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Like {
    /// 乗らない
    Avoid,
    #[default]
    Normal,
    Like,
    Love,
}

impl Like {
    /// 行き先の倍率(飛び移る・画面を移るの点数に掛ける)
    pub fn go(self) -> f64 {
        match self {
            Like::Avoid => 0.0,
            Like::Normal => 1.0,
            Like::Like => 2.0,
            Like::Love => 4.0,
        }
    }

    /// いる時間の倍率(そのウィンドウの上で、次の行動を選ぶまでの時間に掛ける)
    pub fn stay(self) -> f64 {
        match self {
            Like::Avoid | Like::Normal => 1.0,
            Like::Like => 1.5,
            Like::Love => 2.0,
        }
    }
}

/// アプリの好み。名前を複数持てる(SSDをWindowsとMacで共有するが、OSごとにアプリ名が違うため)
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppPref {
    pub names: Vec<String>,
    #[serde(default)]
    pub like: Like,
    /// その上でしやすい行動(点数を3倍にする)
    #[serde(default)]
    pub favorite: Option<Action>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Behavior {
    /// ウィンドウに乗る行動(オフなら今までどおり床の上だけ)
    pub enabled: bool,
    /// 活発さ(1〜5)
    pub activity: u8,
    pub weights: Weights,
    /// 入力中のウィンドウの上では静かにする
    pub quiet_on_focused: bool,
    /// 全画面のアプリが前面のときは、その画面のウィンドウに乗らない
    pub no_ride_fullscreen: bool,
    pub apps: Vec<AppPref>,
}

impl Default for Behavior {
    fn default() -> Self {
        Preset::Normal.behavior()
    }
}

/// 好きな行動の点数の倍率
pub const FAVORITE: f64 = 3.0;

/// ひな形(静か・普通・遊ぶ。設計書の6節の表)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Quiet,
    Normal,
    Play,
}

impl Preset {
    pub fn behavior(self) -> Behavior {
        let (activity, [stand, sit, read, walk, jump, screen, hang, climb, sleep]) = match self {
            Preset::Quiet => (1, [6.0, 6.0, 8.0, 2.0, 1.0, 0.0, 0.0, 0.0, 3.0]),
            Preset::Normal => (3, [5.0, 4.0, 5.0, 4.0, 3.0, 1.0, 1.0, 1.0, 1.0]),
            Preset::Play => (5, [3.0, 3.0, 2.0, 6.0, 7.0, 4.0, 4.0, 4.0, 0.0]),
        };
        Behavior {
            enabled: true,
            activity,
            weights: Weights { stand, sit, read, walk, jump, screen, hang, climb, sleep },
            quiet_on_focused: true,
            no_ride_fullscreen: true,
            apps: Vec::new(),
        }
    }
}

impl Behavior {
    /// 読む。無ければ初めの値(None)。読めなければ初めの値と、理由
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Self::default(), None),
            Err(e) => return (Self::default(), Some(e.to_string())),
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(b) => (b.normalized(), None),
            Err(e) => (Self::default(), Some(e.to_string())),
        }
    }

    /// 範囲の外の値を、範囲の中に収める
    pub fn normalized(mut self) -> Self {
        self.activity = self.activity.clamp(1, 5);
        self.weights = self.weights.clamped();
        self
    }

    /// 動く行動(歩く・飛び移る・画面を移る・ぶら下がる)の点数に掛ける倍率
    pub fn move_factor(&self) -> f64 {
        [0.25, 0.5, 1.0, 2.0, 4.0][(self.activity.clamp(1, 5) - 1) as usize]
    }

    /// 次の行動を選ぶまでの時間に掛ける倍率
    pub fn time_factor(&self) -> f64 {
        [2.0, 1.5, 1.0, 0.75, 0.5][(self.activity.clamp(1, 5) - 1) as usize]
    }

    /// アプリ名から好みを引く(大文字・小文字は区別しない)。登録が無ければNone(普通)
    pub fn pref_of(&self, app: &str) -> Option<&AppPref> {
        self.apps.iter().find(|p| p.names.iter().any(|n| n.eq_ignore_ascii_case(app)))
    }
}

/// 詩織が見かけたアプリ(設定画面で、好みを登録する一覧に並べる。企画書のP-11)。
/// SSDをWindowsとMacで共有するので、どちらのOSで見たかも持つ
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SeenApp {
    pub name: String,
    /// "mac" か "win"
    pub os: String,
    /// 初めて見た日時(ISO 8601)
    pub first_seen: String,
}

/// 見かけたアプリの一覧を読む(無い・読めないときは空)
pub fn load_seen(path: &Path) -> Vec<SeenApp> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// 一覧に無い名前を足す。足したらtrue(同じ名前でもOSが違えば別に並べる)
pub fn merge_seen(list: &mut Vec<SeenApp>, names: &[String], os: &str, now: &str) -> bool {
    let mut changed = false;
    for n in names {
        if n.is_empty() || list.iter().any(|s| s.os == os && s.name == *n) {
            continue;
        }
        list.push(SeenApp { name: n.clone(), os: os.to_string(), first_seen: now.to_string() });
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 見かけたアプリは新しい名前だけ足し_OSが違えば別に並べる() {
        let mut list = Vec::new();
        assert!(merge_seen(&mut list, &["Claude".into(), "Finder".into()], "mac", "t1"));
        assert!(!merge_seen(&mut list, &["Claude".into()], "mac", "t2"));
        assert!(merge_seen(&mut list, &["Claude".into(), String::new()], "win", "t3"));
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].first_seen, "t1");
        assert_eq!(list[2].os, "win");
    }

    #[test]
    fn 書いていない項目は普通のひな形() {
        let b: Behavior = serde_json::from_str(r#"{ "activity": 5, "weights": { "jump": 9 } }"#).unwrap();
        assert_eq!(b.activity, 5);
        assert_eq!(b.weights.jump, 9.0);
        assert_eq!(b.weights.read, Preset::Normal.behavior().weights.read);
        assert!(b.enabled && b.quiet_on_focused && b.no_ride_fullscreen);
    }

    #[test]
    fn 範囲の外の値は収める() {
        let b: Behavior = serde_json::from_str(r#"{ "activity": 9, "weights": { "walk": -3, "jump": 40 } }"#).unwrap();
        let b = b.normalized();
        assert_eq!(b.activity, 5);
        assert_eq!(b.weights.walk, 0.0);
        assert_eq!(b.weights.jump, 10.0);
    }

    #[test]
    fn アプリの好みは名前のどれかで_大文字小文字を区別せず引く() {
        let b: Behavior = serde_json::from_str(
            r#"{ "apps": [ { "names": ["Google Chrome", "chrome"], "like": "love", "favorite": "read" } ] }"#,
        )
        .unwrap();
        let p = b.pref_of("CHROME").unwrap();
        assert_eq!(p.like, Like::Love);
        assert_eq!(p.favorite, Some(Action::Read));
        assert!(b.pref_of("Safari").is_none());
    }

    #[test]
    fn 無いファイルは初めの値_壊れたファイルは初めの値と理由() {
        let dir = std::env::temp_dir().join(format!("shiori-pet-behavior-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (b, err) = Behavior::load(&dir.join("none.json"));
        assert_eq!(b, Behavior::default());
        assert!(err.is_none());
        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{ 壊れている").unwrap();
        let (b, err) = Behavior::load(&bad);
        assert_eq!(b, Behavior::default());
        assert!(err.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 活発さの倍率() {
        let mut b = Behavior::default();
        b.activity = 1;
        assert_eq!((b.move_factor(), b.time_factor()), (0.25, 2.0));
        b.activity = 5;
        assert_eq!((b.move_factor(), b.time_factor()), (4.0, 0.5));
    }
}
