//! libraryのフォルダの並び(置き場の名前)。
//!
//! - `Legacy`: v6.2までの並び。`10-projects`・`20-areas`、全体用のmemoryは `40-profile/memory-global.md`。
//! - `V7`: Ver7.0の並び(P-18・P-19)。`20-projects`(いまの `10-projects` と `20-areas` を合わせたもの)、
//!   `10-common`(全体用のmemoryと全体の日誌)。`type: area` は廃止。
//!
//! 切り替えは移行の日。それまで、アプリ・CLIの直接の動きは `Legacy` のまま、サーバーが `V7` で動く。

use crate::fsx::StoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Legacy,
    V7,
}

/// 全体の日誌(P-19)のproject名。
pub const COMMON_PROJECT: &str = "common";

impl Layout {
    /// 許可するtype。
    pub fn valid_types(self) -> &'static [&'static str] {
        match self {
            Layout::Legacy => &["project", "area", "resource", "profile"],
            Layout::V7 => &["project", "resource", "profile"],
        }
    }

    /// `type: area` を断る(V7)ときの理由。
    pub fn area_rejection(self) -> Option<&'static str> {
        match self {
            Layout::Legacy => None,
            Layout::V7 => Some("type: area は廃止されました。type: project を使ってください"),
        }
    }

    /// project/areaの記事・素材を置くフォルダ(libraryからの相対)。`top`は `project` か `area`。
    pub fn project_dir(self, top: &str, project: &str) -> Result<String, StoreError> {
        match (self, top) {
            (Layout::Legacy, "project") => Ok(format!("10-projects/{project}")),
            (Layout::Legacy, "area") => Ok(format!("20-areas/{project}")),
            (Layout::V7, "project") => Ok(format!("20-projects/{project}")),
            (Layout::V7, "area") => Err(StoreError::Invalid(
                self.area_rejection().unwrap_or("type: area は使えません").to_string(),
            )),
            _ => Err(StoreError::Invalid(format!("typeが不正です: {top}"))),
        }
    }

    /// 全体用のmemoryの置き場所(libraryからの相対)。
    pub fn global_memory_rel(self) -> &'static str {
        match self {
            Layout::Legacy => "40-profile/memory-global.md",
            Layout::V7 => "10-common/memory-global.md",
        }
    }

    /// 日誌を置くフォルダ(libraryからの相対)。`common` は全体の日誌。
    pub fn journal_dir(self, project: &str) -> String {
        if project == COMMON_PROJECT {
            return "10-common/journal".to_string();
        }
        match self {
            Layout::Legacy => format!("10-projects/{project}/journal"),
            Layout::V7 => format!("20-projects/{project}/journal"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 置き場はlayoutごとに決まる() {
        assert_eq!(Layout::Legacy.project_dir("project", "a").unwrap(), "10-projects/a");
        assert_eq!(Layout::Legacy.project_dir("area", "a").unwrap(), "20-areas/a");
        assert_eq!(Layout::V7.project_dir("project", "a").unwrap(), "20-projects/a");
        assert!(Layout::V7.project_dir("area", "a").is_err());
        assert_eq!(Layout::Legacy.global_memory_rel(), "40-profile/memory-global.md");
        assert_eq!(Layout::V7.global_memory_rel(), "10-common/memory-global.md");
        assert_eq!(Layout::V7.journal_dir("a"), "20-projects/a/journal");
        assert_eq!(Layout::V7.journal_dir("common"), "10-common/journal");
    }

    #[test]
    fn v7ではareaを許可しない() {
        assert!(Layout::Legacy.valid_types().contains(&"area"));
        assert!(!Layout::V7.valid_types().contains(&"area"));
        assert!(Layout::V7.area_rejection().is_some() && Layout::Legacy.area_rejection().is_none());
    }
}
