//! ペットの絵(詩織Ver6.0の段階4)。
//!
//! 動きごとに、コマを横に並べた画像(1コマ128×192)と、動きの一覧(`sprites.json`)を持つ。
//! 既定の絵は本体に同梱する(`src/assets/pet/`)。`portable/data/ui/pet/` に同じ名前で置くと、
//! そちらを使う(段階2のアイコンの差し替えと同じ形)。読めない絵や大きさの違う絵は、既定の絵に
//! 戻して `PET-002` をログに書く。動きの絵が無いときは `idle` を使う。
//!
//! クリックを受けるかどうかは、いまのコマのその点が透明かどうかで決めるので、絵ごとに
//! 「不透明な点」の印(mask)を持つ。

use crate::error_codes;
use crate::shiori_log;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub const FRAME_W: u32 = 128;
pub const FRAME_H: u32 = 192;

const DEFAULT_JSON: &str = include_str!("../../../src/assets/pet/sprites.json");
/// 同梱の絵(`sprites.json` に書いた動きの絵は、ここにも足す)
const DEFAULT_FILES: &[(&str, &[u8])] = &[
    ("idle.png", include_bytes!("../../../src/assets/pet/idle.png")),
    ("walk.png", include_bytes!("../../../src/assets/pet/walk.png")),
    ("read.png", include_bytes!("../../../src/assets/pet/read.png")),
    ("sleep.png", include_bytes!("../../../src/assets/pet/sleep.png")),
    ("wake.png", include_bytes!("../../../src/assets/pet/wake.png")),
    ("notify.png", include_bytes!("../../../src/assets/pet/notify.png")),
    ("dangle.png", include_bytes!("../../../src/assets/pet/dangle.png")),
    ("fly.png", include_bytes!("../../../src/assets/pet/fly.png")),
    ("fall.png", include_bytes!("../../../src/assets/pet/fall.png")),
    ("react.png", include_bytes!("../../../src/assets/pet/react.png")),
    ("land.png", include_bytes!("../../../src/assets/pet/land.png")),
    ("happy.png", include_bytes!("../../../src/assets/pet/happy.png")),
    ("appear.png", include_bytes!("../../../src/assets/pet/appear.png")),
    ("jump_on.png", include_bytes!("../../../src/assets/pet/jump_on.png")),
    ("jump_off.png", include_bytes!("../../../src/assets/pet/jump_off.png")),
    ("perch_stand.png", include_bytes!("../../../src/assets/pet/perch_stand.png")),
    ("perch_sit.png", include_bytes!("../../../src/assets/pet/perch_sit.png")),
    ("hang.png", include_bytes!("../../../src/assets/pet/hang.png")),
    ("climb.png", include_bytes!("../../../src/assets/pet/climb.png")),
];

fn default_file(name: &str) -> Option<Vec<u8>> {
    DEFAULT_FILES.iter().find(|(n, _)| *n == name).map(|(_, b)| b.to_vec())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpritesFile {
    frame_width: u32,
    frame_height: u32,
    animations: BTreeMap<String, AnimFile>,
}

#[derive(Deserialize, Clone)]
struct AnimFile {
    file: String,
    frames: u32,
    #[serde(default = "yes")]
    r#loop: bool,
}

fn yes() -> bool {
    true
}

/// 画面へ渡す、動きの一覧(絵そのものは専用のURLの仕組みで渡す)。
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AnimInfo {
    pub file: String,
    pub frames: u32,
    pub r#loop: bool,
}

pub struct Anim {
    pub info: AnimInfo,
    /// 横に並んだ全部のコマの、不透明な点の印(行ごと、幅 = FRAME_W × コマ数)
    mask: Vec<bool>,
    bytes: Vec<u8>,
}

pub struct Sprites {
    pub anims: BTreeMap<String, Anim>,
    /// 差し替えの絵を使っているか
    pub overridden: bool,
}

/// PNGを読み、横に並んだコマの大きさに合っているか確かめて、不透明な点の印を作る。
fn decode(bytes: &[u8], frames: u32) -> Result<Vec<bool>, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut decoder = decoder;
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.width != FRAME_W * frames || info.height != FRAME_H {
        return Err(format!("大きさが{}x{}(期待は{}x{})", info.width, info.height, FRAME_W * frames, FRAME_H));
    }
    let channels = info.color_type.samples();
    let pixels = (info.width * info.height) as usize;
    Ok((0..pixels)
        .map(|i| match channels {
            4 => buf[i * 4 + 3] >= 128,
            2 => buf[i * 2 + 1] >= 128,
            _ => true,
        })
        .collect())
}

impl Sprites {
    /// 差し替えの置き場(`data/ui/pet/`)を見て読む。読めなければ既定の絵。
    pub fn load(override_dir: &Path) -> Self {
        if override_dir.join("sprites.json").is_file() {
            match Self::load_from(|f| std::fs::read(override_dir.join(f)).ok()) {
                Ok(mut s) => {
                    s.overridden = true;
                    shiori_log::info("PET", "差し替えの絵を読んだ", &[("動き", &s.anims.len())]);
                    return s;
                }
                Err(e) => shiori_log::warn(
                    error_codes::PET_002.code,
                    "差し替えの絵を読めないので、既定の絵に戻した",
                    &[("理由", &e)],
                ),
            }
        }
        Self::load_from(default_file).expect("同梱のペットの絵が読めない")
    }

    fn load_from(read: impl Fn(&str) -> Option<Vec<u8>>) -> Result<Self, String> {
        let text = match read("sprites.json") {
            Some(b) => String::from_utf8(b).map_err(|e| e.to_string())?,
            None => DEFAULT_JSON.to_string(),
        };
        let file: SpritesFile = serde_json::from_str(&text).map_err(|e| format!("sprites.json: {e}"))?;
        if file.frame_width != FRAME_W || file.frame_height != FRAME_H {
            return Err(format!("1コマの大きさは{FRAME_W}x{FRAME_H}にする"));
        }
        let mut anims = BTreeMap::new();
        for (name, a) in file.animations {
            if a.frames == 0 || !crate::hub::manifest::is_valid_name(a.file.trim_end_matches(".png")) {
                return Err(format!("{name}: fileかframesが正しくない"));
            }
            let bytes = read(&a.file).ok_or_else(|| format!("{name}: {}が無い", a.file))?;
            let mask = decode(&bytes, a.frames).map_err(|e| format!("{name}: {e}"))?;
            anims.insert(name, Anim { info: AnimInfo { file: a.file.clone(), frames: a.frames, r#loop: a.r#loop }, mask, bytes });
        }
        if !anims.contains_key("idle") {
            return Err("idleの絵が要る".into());
        }
        Ok(Self { anims, overridden: false })
    }

    /// 動きの絵。無ければ `idle`。
    pub fn anim(&self, name: &str) -> (&str, &Anim) {
        // ふらるが差し替えた絵の組に新しい動きの絵が無いときは、近い動きの絵を使う(ペットの行動の設計書の7節)
        let near = match name {
            "jump_on" => "fly",
            "jump_off" => "fall",
            "hang" | "climb" => "dangle",
            _ => "idle",
        };
        for n in [name, near] {
            if let Some((k, a)) = self.anims.get_key_value(n) {
                return (k.as_str(), a);
            }
        }
        ("idle", &self.anims["idle"])
    }

    /// そのコマの、その点(ドット、左上から)が不透明か。左向きのときは左右を反転して見る。
    pub fn opaque_at(&self, anim: &str, frame: u32, facing_left: bool, dx: i32, dy: i32) -> bool {
        if dx < 0 || dy < 0 || dx >= FRAME_W as i32 || dy >= FRAME_H as i32 {
            return false;
        }
        let (_, a) = self.anim(anim);
        let frame = frame % a.info.frames;
        let x = if facing_left { FRAME_W as i32 - 1 - dx } else { dx };
        let width = (FRAME_W * a.info.frames) as usize;
        a.mask[dy as usize * width + (frame * FRAME_W) as usize + x as usize]
    }

    /// 絵のファイル(専用のURLの仕組みから返す)
    pub fn file_bytes(&self, file: &str) -> Option<Vec<u8>> {
        self.anims.values().find(|a| a.info.file == file).map(|a| a.bytes.clone())
    }

    pub fn infos(&self) -> BTreeMap<String, AnimInfo> {
        self.anims.iter().map(|(k, a)| (k.clone(), a.info.clone())).collect()
    }

    /// 不透明な点のうち、いちばん中ほどにあるもの(自己確認で「絵の上の点」に使う)
    pub fn center_opaque(&self) -> Option<(i32, i32)> {
        let (cx, cy) = (FRAME_W as i32 / 2, FRAME_H as i32 / 2);
        (0..FRAME_H as i32)
            .flat_map(|y| (0..FRAME_W as i32).map(move |x| (x, y)))
            .filter(|&(x, y)| self.opaque_at("idle", 0, false, x, y))
            .min_by_key(|&(x, y)| (x - cx).pow(2) + (y - cy).pow(2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 同梱の絵は読め_外と真ん中で透明かどうかが分かる() {
        let s = Sprites::load_from(default_file).unwrap();
        assert!(!s.opaque_at("idle", 0, false, 0, 0));
        let (x, y) = s.center_opaque().expect("不透明な点がある");
        assert!(s.opaque_at("idle", 0, false, x, y));
        // 無い動きはidleで出す
        assert_eq!(s.anim("no-such-anim").0, "idle");
        assert!(!s.opaque_at("idle", 0, false, -1, 5));
    }

    #[test]
    fn 大きさの違う絵は読まない() {
        let json = r#"{"frameWidth":128,"frameHeight":192,"animations":{"idle":{"file":"idle.png","frames":2}}}"#;
        let r = Sprites::load_from(|f| match f {
            "sprites.json" => Some(json.as_bytes().to_vec()),
            "idle.png" => default_file("idle.png"),
            _ => None,
        });
        assert!(r.is_err());
    }

    #[test]
    fn 同梱の動きの絵は全部読め_無い動きはidleになる() {
        let s = Sprites::load_from(default_file).unwrap();
        for name in ["idle", "walk", "read", "sleep", "wake"] {
            assert_eq!(s.anim(name).0, name);
        }
        assert_eq!(s.anim("read").1.info.frames, 32);
        assert_eq!(s.anim("walk").1.info.frames, 24);
        assert!(!s.anim("wake").1.info.r#loop);
        assert_eq!(s.anim("no-such-anim").0, "idle");
    }
}
