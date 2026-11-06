//! 埋め込みのサーバー(llama-server --embedding、既定は8082)への問い合わせ。
//! `services/rag/embedding_client.py` と同じく、nomic-embed-textの決まりの前置きを付ける。

use std::time::Duration;

use serde::Deserialize;

use crate::log;

pub struct Embedder {
    url: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct Resp {
    data: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    embedding: Vec<f32>,
}

impl Embedder {
    pub fn new(port: u16) -> Self {
        Self {
            url: format!("http://127.0.0.1:{port}/v1/embeddings"),
            agent: ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).build(),
        }
    }

    /// 質問は `search_query: `、文書は `search_document: ` を前に付けて埋め込む。
    pub fn embed(&self, text: &str, is_query: bool) -> Result<Vec<f32>, String> {
        let prefix = if is_query { "search_query: " } else { "search_document: " };
        let body = serde_json::json!({ "input": format!("{prefix}{text}") });
        let response = match self.post(&body) {
            // 埋め込みのサーバーは空いた接続を閉じる。Macでは、閉じられた接続を再利用すると
            // 接続のリセットで失敗し(RAG-013)、ureqはこれをやり直さない。失敗した接続は
            // 捨てられるので、1回だけやり直す。埋め込みは何度送っても害がない。
            // HTTPのエラー(サーバーが応答を返した場合)は、やり直しても同じなので対象にしない。
            Err(ureq::Error::Transport(e)) => {
                log::warn("RAG", "埋め込みのサーバーとの接続が切れたので、やり直す", &[("理由", &e)]);
                self.post(&body)
            }
            result => result,
        };
        let resp: Resp = response
            .map_err(|e| format!("埋め込みのサーバーに問い合わせられない: {e}"))?
            .into_json()
            .map_err(|e| format!("埋め込みの応答を読めない: {e}"))?;
        resp.data.into_iter().next().map(|i| i.embedding).ok_or_else(|| "埋め込みの応答が空".to_string())
    }

    fn post(&self, body: &serde_json::Value) -> Result<ureq::Response, ureq::Error> {
        self.agent.post(&self.url).send_json(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread::JoinHandle;

    /// 接続ごとに `replies` の応答を順に返す偽のサーバー。`None` は応答せずに接続を切る。
    /// 終わると、受けた接続の数を返す。
    fn fake_server(replies: Vec<Option<&'static str>>) -> (u16, JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let count = replies.len();
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                if let Some(reply) = reply {
                    stream.write_all(reply.as_bytes()).unwrap();
                }
            }
            count
        });
        (port, handle)
    }

    /// 要求を最後まで読む。読み残して閉じると、応答の前に接続がリセットされるため。
    fn read_request(stream: &mut TcpStream) {
        let mut received = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            received.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&received).to_lowercase();
            if let Some(end) = text.find("\r\n\r\n") {
                let length: usize = text
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if n == 0 || received.len() >= end + 4 + length {
                    return;
                }
            }
            if n == 0 {
                return;
            }
        }
    }

    const OK: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 35\r\nConnection: close\r\n\r\n{\"data\":[{\"embedding\":[0.5,0.25]}]}";
    const SERVER_ERROR: &str = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    #[test]
    fn 接続が切れたら1回だけやり直して成功する() {
        let (port, server) = fake_server(vec![None, Some(OK)]);
        let result = Embedder::new(port).embed("確認", true);
        assert_eq!(result.unwrap(), vec![0.5, 0.25]);
        assert_eq!(server.join().unwrap(), 2);
    }

    #[test]
    fn やり直しても切れたらエラーを返す() {
        let (port, server) = fake_server(vec![None, None]);
        let error = Embedder::new(port).embed("確認", true).unwrap_err();
        assert!(error.contains("問い合わせられない"), "{error}");
        assert_eq!(server.join().unwrap(), 2);
    }

    #[test]
    fn サーバーがエラーを返したらやり直さない() {
        // 応答は1つだけ用意する。やり直すと、接続先が無くなって別のエラーになる。
        let (port, server) = fake_server(vec![Some(SERVER_ERROR)]);
        let error = Embedder::new(port).embed("確認", true).unwrap_err();
        assert!(error.contains("500"), "{error}");
        assert_eq!(server.join().unwrap(), 1);
    }
}
