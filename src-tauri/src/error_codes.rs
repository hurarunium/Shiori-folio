//! エラーコードの一覧(詩織Ver6.0の段階1)。
//!
//! 形は「部品名-3桁の番号」(例: `RAG-012`)。外部のAIが修復するときの手がかりにするため、
//! ログと画面に同じコードを出す。本体のコードは、ここに番号と意味を並べて持つ(コードと
//! 一覧が離れないようにする)。拡張機能のコードは、それぞれの説明書(`extension.json`)に書く。
//!
//! 部品名: APP(本体の全体)・LOG・CFG(設定)・HUB(ハブ)・EXT(拡張機能の読み込み・承認・窓口)・
//! LLM・EMB(埋め込み)・RAG・STT(音声認識)・TTS(読み上げ)・MCP・SAVE・EJECT・LIB(library)・SHELF(棚)・
//! UI(本体の画面)・PET(ペットの形)・NOTIFY(知らせと時刻の予約。コードは使わず、部品名としてだけ使う)・SELFTEST(自己確認の起動。コードは使わず、部品名としてだけ使う)。
//! いまの機能のエラーには、手を入れるときに順に番号を付ける(段階1では一斉には付けない)。

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorCode {
    pub code: &'static str,
    pub message: &'static str,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code)
    }
}

macro_rules! codes {
    ($($name:ident = $code:literal, $message:literal;)*) => {
        $(pub const $name: ErrorCode = ErrorCode { code: $code, message: $message };)*
        /// 全部のコード(一覧の画面や、重なりのテストに使う)
        pub const ALL: &[ErrorCode] = &[$($name),*];
    };
}

codes! {
    // ログ(shiori_log.rsの中では、初期化の前なので文字列で直接出している)
    LOG_001 = "LOG-001", "ログのフォルダを作れない";
    LOG_002 = "LOG-002", "ログのファイルを開けない";
    // 起動時の自動修復(段階6、repair.rs)。直したものは部品名FIXの行でログに書く
    APP_001 = "APP-001", "作業用のフォルダが無かったので作り直した";
    APP_002 = "APP-002", "古いロック(記録のPIDが動いていない)を消した";
    APP_003 = "APP-003", "索引のフォルダが無いので、RAGの起動の同期で作り直す";
    APP_004 = "APP-004", "作業用のフォルダかロックを直せなかった";
    CFG_001 = "CFG-001", "config.jsonが壊れていたので脇へ退け、前回正常に起動したときの写しに戻した";
    CFG_002 = "CFG-002", "config.jsonが壊れていて前回の写しも無いので、脇へ退けて既定の設定に戻した";
    CFG_003 = "CFG-003", "壊れたconfig.jsonを直せなかった(書けないなど)";
    CFG_004 = "CFG-004", "前回正常に起動したときのconfig.jsonの写しを書けない";
    // ハブ(拡張機能の置き場の全体)
    HUB_001 = "HUB-001", "拡張機能のフォルダを読めない";
    // 拡張機能の説明書と承認
    EXT_001 = "EXT-001", "説明書(extension.json)が無い";
    EXT_002 = "EXT-002", "説明書がJSONとして読めないか、知らない項目・足りない項目がある";
    EXT_003 = "EXT-003", "説明書のnameがフォルダの名前と違う";
    EXT_004 = "EXT-004", "説明書の項目の値が正しくない";
    EXT_005 = "EXT-005", "説明書が指すファイル(入口・アイコン)が無いか、フォルダの外を指している";
    EXT_006 = "EXT-006", "説明書のusesに、詩織が貸さないものが書かれている";
    EXT_007 = "EXT-007", "承認の記録を読めないか、書けない";
    EXT_008 = "EXT-008", "その名前の拡張機能が無い";
    // 拡張機能からの頼みごと(窓口)と、画面の部品の配り方
    EXT_009 = "EXT-009", "拡張機能が使える状態ではない(無いか、承認されていない)";
    EXT_010 = "EXT-010", "詩織が知らない頼みごと";
    EXT_011 = "EXT-011", "説明書のusesに書いていないものを頼んだ";
    EXT_012 = "EXT-012", "まだ使えない頼みごと(これから作る)";
    EXT_013 = "EXT-013", "拡張機能のファイルを返せない(無い、承認されていない、フォルダの外など)";
    EXT_014 = "EXT-014", "頼みごとの中身(params)が正しくない";
    // 別のプログラムの拡張機能
    EXT_015 = "EXT-015", "別のプログラムの拡張機能が止まったので、再起動する";
    EXT_016 = "EXT-016", "別のプログラムの拡張機能が何度も止まるので、再起動をやめた";
    EXT_017 = "EXT-017", "別のプログラムの拡張機能が動いていないか、時間内に返事をしない";
    EXT_018 = "EXT-018", "別のプログラムの拡張機能を起動できない";
    EXT_019 = "EXT-019", "別のプログラムの拡張機能から届いた行が、決まりの形のJSONではない";
    EXT_020 = "EXT-020", "相手の拡張機能の説明書のprovidesに無い操作は呼べない";
    // 拡張機能に貸すもの
    EXT_021 = "EXT-021", "記事の閲覧で、libraryと棚の外か、.md・.meta以外を読もうとした";
    EXT_022 = "EXT-022", "拡張機能の置き場を読み書きできない(壊れている、大きすぎるなど)";
    EXT_023 = "EXT-023", "拡張機能からの検索に失敗した(RAGが動いていないなど)";
    // 段階3の窓口(クリップボード・時刻の予約)
    EXT_024 = "EXT-024", "クリップボードに書けない";
    EXT_025 = "EXT-025", "予約の数が多すぎる(拡張機能ごとに20件まで)";
    EXT_026 = "EXT-026", "その予約が無い(もう終わったか、ほかの拡張機能の予約)";
    // RAG(段階5のRust版 shiori-rag。一覧の元は services/shiori-rag/src/errors.rs。そちらと同じに保つ)
    RAG_001 = "RAG-001", "設定(config.json)を読めない";
    RAG_002 = "RAG-002", "索引のフォルダを作れないか、読めない";
    RAG_003 = "RAG-003", "索引のファイルが壊れている(その記事は作り直す)";
    RAG_004 = "RAG-004", "索引のファイルを書けない";
    RAG_005 = "RAG-005", "記事を読めない(文字コードなど)ので、索引に入れずに飛ばした";
    RAG_006 = "RAG-006", "埋め込みのサーバーに問い合わせられない";
    RAG_007 = "RAG-007", "リランカーのモデルを読み込めない";
    RAG_008 = "RAG-008", "リランカーの計算に失敗した";
    RAG_009 = "RAG-009", "素材に秘密情報らしき文字列があるので、抽出を止めた";
    RAG_010 = "RAG-010", "素材の抽出に失敗した";
    RAG_011 = "RAG-011", "ポートを開けない";
    RAG_012 = "RAG-012", "要求の形が正しくない";
    RAG_013 = "RAG-013", "要求の処理に失敗した";
    // ペットの形(段階4)
    PET_001 = "PET-001", "ペットのウィンドウ(本・吹き出しを含む)を作れないので、ウィンドウの形のままにした";
    PET_002 = "PET-002", "ペットの絵を読めないか、大きさが違うので、既定の絵に戻した";
    PET_003 = "PET-003", "形の切り替えのホットキーを登録できない(ボタンからは切り替えられる)";
    PET_004 = "PET-004", "無操作の秒数を取れないので、眠る動きを止めた";
    PET_005 = "PET-005", "ほかのアプリのウィンドウを読めないので、床の上だけで動く";
    PET_006 = "PET-006", "ペットの行動の設定(data/pet/behavior.json)を読めないので、普通のひな形で動く";
    // 本体の画面(段階2)
    UI_001 = "UI-001", "列の並びの記録(sidebar.json)が壊れていたので、脇に退けて既定の並びにした";
    UI_002 = "UI-002", "列の並びの記録(sidebar.json)を書けない";
}

/// コードから意味を引く。
pub fn lookup(code: &str) -> Option<&'static ErrorCode> {
    ALL.iter().find(|c| c.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn コードは重ならず形がそろっている() {
        let mut seen = std::collections::HashSet::new();
        for c in ALL {
            assert!(seen.insert(c.code), "重なり: {}", c.code);
            let (part, num) = c.code.rsplit_once('-').expect("部品名-番号の形");
            assert!(part.chars().all(|ch| ch.is_ascii_uppercase()), "部品名: {}", c.code);
            assert!(num.len() == 3 && num.chars().all(|ch| ch.is_ascii_digit()), "番号: {}", c.code);
        }
        assert_eq!(lookup("LOG-001").map(|c| c.message), Some("ログのフォルダを作れない"));
    }

    #[test]
    fn RAGのコードはshiori_ragの一覧と同じ() {
        // shiori-ragは別のクレートなので、一覧のファイルを文字列として読んで突き合わせる
        let theirs = include_str!("../../services/shiori-rag/src/errors.rs");
        let ours: Vec<_> = ALL.iter().filter(|c| c.code.starts_with("RAG-")).collect();
        let their_count = theirs.matches("= \"RAG-").count();
        assert_eq!(ours.len(), their_count, "数が違う");
        for c in ours {
            let line = format!("= \"{}\", \"{}\";", c.code, c.message);
            assert!(theirs.contains(&line), "shiori-ragの一覧と違う: {}", c.code);
        }
    }
}
