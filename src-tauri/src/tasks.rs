//! タスクの読み書きは `library_store::tasks` へ移した(Ver7.0。サーバーとアプリ・CLIが同じ処理を使うため)。
//! ここは、これまでの `tasks::…` の呼び出しをそのまま通すための窓口。

pub use library_store::tasks::*;
