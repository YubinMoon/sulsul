//! 테스트용 가짜 HTTP 서버. 요청을 차례로 받아 정해 둔 답을 하나씩 돌려줍니다.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use serde_json::Value;

/// 가짜 서버가 돌려줄 답 하나
pub(crate) struct Reply {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: String,
}

/// JSON 본문으로 답합니다.
pub(crate) fn json(status: &'static str, body: impl Into<String>) -> Reply {
    Reply {
        status,
        content_type: "application/json",
        body: body.into(),
    }
}

/// 서버 전송 이벤트(SSE) 본문으로 답합니다.
pub(crate) fn sse(body: impl Into<String>) -> Reply {
    Reply {
        status: "200 OK",
        content_type: "text/event-stream",
        body: body.into(),
    }
}

/// 요청을 `replies` 개수만큼 받아 차례로 답하는 서버를 띄웁니다.
/// 서버 주소(`http://127.0.0.1:포트`)와, 받은 요청 원문들을 돌려줄 손잡이를 돌려줍니다.
pub(crate) fn serve(replies: Vec<Reply>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        replies
            .into_iter()
            .map(|reply| {
                let (mut stream, _) = listener.accept().unwrap();
                let raw = read_http_request(&mut stream);
                let response = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    reply.status,
                    reply.content_type,
                    reply.body.len(),
                    reply.body
                );
                stream.write_all(response.as_bytes()).unwrap();
                raw
            })
            .collect()
    });
    (format!("http://{addr}"), server)
}

/// 머리글과 `Content-Length`만큼의 본문을 읽습니다.
pub(crate) fn read_http_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut expected_total: Option<usize> = None;
    loop {
        let n = stream.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if expected_total.is_none()
            && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
        {
            let headers = String::from_utf8_lossy(&buf[..pos]);
            let content_len = headers
                .lines()
                .find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            expected_total = Some(pos + 4 + content_len);
        }
        if let Some(total) = expected_total
            && buf.len() >= total
        {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// 요청 원문의 JSON 본문
pub(crate) fn body_of(raw: &str) -> Value {
    serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

/// 요청 원문의 머리글 하나 (이름은 대소문자를 가리지 않음)
pub(crate) fn header_of<'a>(raw: &'a str, name: &str) -> Option<&'a str> {
    raw.split("\r\n\r\n").next()?.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}
