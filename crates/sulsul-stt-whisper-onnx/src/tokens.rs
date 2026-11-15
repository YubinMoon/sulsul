//! Whisper 토큰 표(`*-tokens.txt`)와 토큰 → 텍스트 변환.
//!
//! sherpa-onnx가 내보낸 토큰 파일은 줄마다 `<토큰 바이트의 base64> <토큰 ID>`입니다(tiktoken의 BPE 순위 표).
//! 특수 토큰(`<|endoftext|>`, 언어, 타임스탬프 등)은 파일에 없고 ID는 모델 메타데이터에 있습니다.
//! 한 글자(UTF-8 여러 바이트)가 토큰 여러 개로 나뉠 수 있으므로 바이트를 모두 이어 붙인 뒤 문자열로 바꿉니다.

use std::path::Path;

/// 토큰 ID → 바이트 표
#[derive(Debug, Clone, Default)]
pub struct TokenTable {
    bytes: Vec<Option<Box<[u8]>>>,
}

impl TokenTable {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut bytes: Vec<Option<Box<[u8]>>> = Vec::new();
        for (number, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (encoded, id) = line
                .rsplit_once(char::is_whitespace)
                .ok_or_else(|| format!("line {}: expected '<base64> <id>'", number + 1))?;
            let id: usize = id
                .parse()
                .map_err(|e| format!("line {}: bad token id: {e}", number + 1))?;
            let decoded = decode_base64(encoded.trim())
                .ok_or_else(|| format!("line {}: bad base64", number + 1))?;
            if bytes.len() <= id {
                bytes.resize(id + 1, None);
            }
            bytes[id] = Some(decoded.into_boxed_slice());
        }
        if bytes.is_empty() {
            return Err("token file is empty".into());
        }
        Ok(Self { bytes })
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.bytes.iter().filter(|b| b.is_some()).count()
    }

    /// 텍스트 토큰을 이어 붙여 문자열로 바꿉니다. 표에 없는 ID(특수 토큰)는 건너뜁니다.
    /// 끝까지 이어 붙여도 올바른 UTF-8이 아닌 바이트는 `U+FFFD`로 바꿉니다.
    pub fn decode(&self, ids: &[i64]) -> String {
        let mut out = Vec::new();
        for &id in ids {
            if let Some(Some(bytes)) = usize::try_from(id).ok().and_then(|i| self.bytes.get(i)) {
                out.extend_from_slice(bytes);
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

/// 표준 base64(`A-Z a-z 0-9 + /`, `=` 채움)를 풉니다. 잘못된 문자가 있으면 `None`입니다.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let body = text.trim_end_matches('=');
    if text.len() - body.len() > 2 || body.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    for chunk in body.as_bytes().chunks(4) {
        let mut acc = 0_u32;
        for &c in chunk {
            acc = (acc << 6) | value(c)?;
        }
        // 4글자 = 3바이트, 3글자 = 2바이트, 2글자 = 1바이트
        acc <<= 6 * (4 - chunk.len() as u32);
        let produced = chunk.len() * 3 / 4;
        out.extend_from_slice(&acc.to_be_bytes()[1..1 + produced]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_known_values() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
        assert_eq!(decode_base64("Zm8=").unwrap(), b"fo");
        assert_eq!(decode_base64("Zm9v").unwrap(), b"foo");
        assert_eq!(decode_base64("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(decode_base64("IGhlbGxv").unwrap(), b" hello");
        // "안" = EC 95 88, 바이트 하나짜리 토큰 "7A=="
        assert_eq!(decode_base64("7JWI").unwrap(), "안".as_bytes());
        assert_eq!(decode_base64("7A==").unwrap(), [0xEC]);
        assert_eq!(decode_base64("+/8=").unwrap(), [0xFB, 0xFF]);
        assert!(decode_base64("Zm9v!").is_none());
        assert!(decode_base64("Z").is_none());
        assert!(decode_base64("Zg===").is_none());
    }

    #[test]
    fn decode_joins_bytes_split_across_tokens() {
        // "안녕" = EC 95 88 / EB 85 95. 첫 글자를 두 토큰으로 나눕니다.
        let table = TokenTable::parse("IA== 220\n7JU= 1\niOs= 2\nhZU= 3\nIGhp 4\n").unwrap();
        assert_eq!(table.len(), 5);
        assert_eq!(table.decode(&[220, 1, 2, 3]), " 안녕");
        // 특수 토큰처럼 표에 없는 ID와 음수 ID는 건너뜁니다.
        assert_eq!(table.decode(&[50258, 4, -1, 99_999]), " hi");
        // 끝까지 맞춰지지 않는 바이트는 대체 문자로 바꿉니다.
        assert_eq!(table.decode(&[1]), "\u{FFFD}");
    }

    #[test]
    fn parse_rejects_malformed_lines() {
        assert!(TokenTable::parse("").is_err());
        assert!(TokenTable::parse("IA==").is_err());
        assert!(TokenTable::parse("IA== x").is_err());
        assert!(TokenTable::parse("!!!! 1").is_err());
    }
}
