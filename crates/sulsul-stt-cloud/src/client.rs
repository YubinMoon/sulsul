//! OpenAI 호환 `POST /audio/transcriptions` HTTP(S) 클라이언트.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;
use sulsul_core::audio::{AudioClip, encode_wav_bytes};
use sulsul_core::stt::{
    ComputeDevice, Segment, SttEngine, SttError, TranscribeRequest, Transcript,
};

static BOUNDARY_SEQ: AtomicU64 = AtomicU64::new(1);

pub struct CloudConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub timeout: Duration,
}

pub struct CloudEngine {
    agent: ureq::Agent,
    transcriptions_url: String,
    api_key: String,
    model: String,
}

impl CloudEngine {
    pub fn new(config: CloudConfig) -> Result<Self, SttError> {
        let transcriptions_url = normalize_transcriptions_url(&config.endpoint)?;
        let model = config.model.trim().to_owned();
        if model.is_empty() {
            return Err(SttError::Backend(
                "API 모델 이름이 비어 있습니다. 모델 관리 탭에서 모델 이름을 입력하세요.".into(),
            ));
        }

        let agent_config = ureq::Agent::config_builder()
            .timeout_global(Some(config.timeout))
            .http_status_as_error(false)
            .build();
        Ok(Self {
            agent: agent_config.into(),
            transcriptions_url,
            api_key: config.api_key.trim().to_owned(),
            model,
        })
    }
}

impl SttEngine for CloudEngine {
    fn device(&self) -> ComputeDevice {
        ComputeDevice::Unknown
    }

    fn transcribe(
        &mut self,
        clip: &AudioClip,
        request: &TranscribeRequest,
    ) -> Result<Transcript, SttError> {
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }
        if clip.is_empty() {
            return Ok(Transcript::default());
        }

        let wav_bytes = encode_wav_bytes(clip)
            .map_err(|e| SttError::Backend(format!("WAV 인코딩 실패: {e}")))?;
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }

        let seq = BOUNDARY_SEQ.fetch_add(1, Ordering::Relaxed);
        let boundary = format!("----sulsul-multipart-{seq}");
        let body = build_multipart_body(
            &boundary,
            &wav_bytes,
            &self.model,
            request.language.code(),
            request.prompt.as_deref(),
        );

        let mut req = self.agent.post(&self.transcriptions_url).header(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        );
        if !self.api_key.is_empty() {
            req = req.header("Authorization", &format!("Bearer {}", self.api_key));
        }

        let mut response = req
            .send(&body[..])
            .map_err(|e| SttError::Backend(format!("API 요청 실패: {e}")))?;
        if request.cancel.is_cancelled() {
            return Err(SttError::Cancelled);
        }

        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| SttError::Backend(format!("API 응답을 읽지 못했습니다: {e}")))?;

        if !(200..300).contains(&status) {
            return Err(SttError::Backend(format_http_error(status, &text)));
        }

        parse_transcription_response(&text)
    }
}

/// 사용자가 넣은 기본 URL이나 전체 URL을 `/audio/transcriptions` 엔드포인트로 정규화합니다.
pub fn normalize_transcriptions_url(raw: &str) -> Result<String, SttError> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(SttError::Backend(
            "API 엔드포인트가 비어 있습니다. 모델 관리 탭에서 API 프로필을 설정하세요.".into(),
        ));
    }
    let after_scheme = trimmed
        .strip_prefix("https://")
        .or_else(|| trimmed.strip_prefix("http://"))
        .ok_or_else(|| {
            SttError::Backend(format!(
                "API 엔드포인트는 http:// 또는 https://로 시작해야 합니다: {trimmed}"
            ))
        })?;
    let host = after_scheme.split('/').next().unwrap_or_default();
    if host.is_empty() {
        return Err(SttError::Backend(format!(
            "올바른 API 엔드포인트 주소가 아닙니다: {trimmed}"
        )));
    }
    if trimmed.ends_with("/audio/transcriptions") {
        return Ok(trimmed.to_owned());
    }
    if after_scheme.contains('/') {
        Ok(format!("{trimmed}/audio/transcriptions"))
    } else {
        Ok(format!("{trimmed}/v1/audio/transcriptions"))
    }
}

fn build_multipart_body(
    boundary: &str,
    wav_bytes: &[u8],
    model: &str,
    language: Option<&str>,
    prompt: Option<&str>,
) -> Vec<u8> {
    let mut body = Vec::with_capacity(wav_bytes.len() + 512);

    write_text_part(&mut body, boundary, "model", model);
    write_text_part(&mut body, boundary, "response_format", "json");
    if let Some(lang) = language.map(str::trim).filter(|s| !s.is_empty()) {
        write_text_part(&mut body, boundary, "language", lang);
    }
    if let Some(p) = prompt.map(str::trim).filter(|s| !s.is_empty()) {
        write_text_part(&mut body, boundary, "prompt", p);
    }

    let mut header = String::new();
    let _ = write!(
        header,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
    );
    body.extend_from_slice(header.as_bytes());
    body.extend_from_slice(wav_bytes);
    body.extend_from_slice(b"\r\n");

    let _ = write!(body_end_buf(&mut body), "--{boundary}--\r\n");
    body
}

fn body_end_buf(buf: &mut Vec<u8>) -> impl std::fmt::Write + '_ {
    struct VecWrite<'a>(&'a mut Vec<u8>);
    impl std::fmt::Write for VecWrite<'_> {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }
    VecWrite(buf)
}

fn write_text_part(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    let mut head = String::new();
    let _ = write!(
        head,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n"
    );
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    text: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    segments: Vec<ApiSegment>,
}

#[derive(Deserialize)]
struct ApiSegment {
    #[serde(default)]
    start: f64,
    #[serde(default)]
    end: f64,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    message: String,
}

fn parse_transcription_response(raw: &str) -> Result<Transcript, SttError> {
    let parsed: ApiResponse = serde_json::from_str(raw).map_err(|e| {
        SttError::Backend(format!(
            "API 응답 JSON 형식을 해석하지 못했습니다 ({e}): {}",
            truncate_for_log(raw)
        ))
    })?;
    let segments = parsed
        .segments
        .into_iter()
        .map(|s| Segment {
            start: Duration::from_secs_f64(s.start.max(0.0)),
            end: Duration::from_secs_f64(s.end.max(s.start).max(0.0)),
            text: s.text.trim().to_owned(),
        })
        .collect();
    Ok(Transcript {
        text: parsed.text.trim().to_owned(),
        language: parsed.language.filter(|s| !s.trim().is_empty()),
        segments,
    })
}

fn format_http_error(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<ApiErrorEnvelope>(body)
        .ok()
        .map(|env| env.error.message.trim().to_owned())
        .filter(|msg| !msg.is_empty())
        .unwrap_or_else(|| truncate_for_log(body));

    if detail.is_empty() {
        format!("API 오류 (HTTP {status})")
    } else {
        format!("API 오류 (HTTP {status}): {detail}")
    }
}

fn truncate_for_log(raw: &str) -> String {
    let trimmed = raw.trim();
    const MAX: usize = 200;
    if trimmed.chars().count() <= MAX {
        return trimmed.to_owned();
    }
    let prefix: String = trimmed.chars().take(MAX).collect();
    format!("{prefix}...")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use sulsul_core::CancelToken;
    use sulsul_core::audio::test_audio_clip;
    use sulsul_core::stt::Language;

    #[test]
    fn normalizes_common_endpoint_shapes() {
        assert_eq!(
            normalize_transcriptions_url("https://api.openai.com/v1").unwrap(),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_transcriptions_url("https://api.groq.com/openai/v1/").unwrap(),
            "https://api.groq.com/openai/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_transcriptions_url("http://127.0.0.1:8000").unwrap(),
            "http://127.0.0.1:8000/v1/audio/transcriptions"
        );
        assert_eq!(
            normalize_transcriptions_url("https://proxy.example.com/v1/audio/transcriptions")
                .unwrap(),
            "https://proxy.example.com/v1/audio/transcriptions"
        );
        assert!(normalize_transcriptions_url("ftp://bad").is_err());
        assert!(normalize_transcriptions_url("  ").is_err());
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
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

    #[test]
    fn cloud_engine_sends_multipart_and_parses_transcript() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let raw = read_http_request(&mut stream);
            assert!(raw.starts_with("POST /v1/audio/transcriptions HTTP/1.1"));
            let lower = raw.to_ascii_lowercase();
            assert!(lower.contains("authorization: bearer sk-test-123"));
            assert!(lower.contains("content-type: multipart/form-data; boundary="));
            assert!(raw.contains("name=\"model\"\r\n\r\nwhisper-1"));
            assert!(raw.contains("name=\"language\"\r\n\r\nko"));
            assert!(raw.contains("name=\"prompt\"\r\n\r\n술술 테스트"));
            assert!(raw.contains("filename=\"audio.wav\""));
            assert!(raw.contains("RIFF"));

            let payload = r#"{"text":" 테스트 음성 인식 ","language":"ko","segments":[{"start":0.0,"end":1.2,"text":"테스트 음성 인식"}]}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let mut engine = CloudEngine::new(CloudConfig {
            endpoint: format!("http://{addr}/v1"),
            api_key: "sk-test-123".into(),
            model: "whisper-1".into(),
            timeout: Duration::from_secs(5),
        })
        .unwrap();

        let clip = test_audio_clip(16_000);
        let transcript = engine
            .transcribe(
                &clip,
                &TranscribeRequest {
                    language: Language::Code("ko".into()),
                    prompt: Some("술술 테스트".into()),
                    cancel: CancelToken::new(),
                },
            )
            .unwrap();
        server.join().unwrap();

        assert_eq!(transcript.text, "테스트 음성 인식");
        assert_eq!(transcript.language.as_deref(), Some("ko"));
        assert_eq!(transcript.segments.len(), 1);
    }

    #[test]
    fn cloud_engine_reports_openai_error_json() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_http_request(&mut stream);
            let payload = r#"{"error":{"message":"Incorrect API key provided"}}"#;
            let resp = format!(
                "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            stream.write_all(resp.as_bytes()).unwrap();
        });

        let mut engine = CloudEngine::new(CloudConfig {
            endpoint: format!("http://{addr}/v1"),
            api_key: "bad".into(),
            model: "whisper-1".into(),
            timeout: Duration::from_secs(5),
        })
        .unwrap();

        let err = engine
            .transcribe(
                &test_audio_clip(16_000),
                &TranscribeRequest {
                    language: Language::Auto,
                    prompt: None,
                    cancel: CancelToken::new(),
                },
            )
            .unwrap_err();
        server.join().unwrap();

        let message = err.to_string();
        assert!(message.contains("401"));
        assert!(message.contains("Incorrect API key provided"));
    }
}
