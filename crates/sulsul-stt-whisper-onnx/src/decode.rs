//! 디코더 출력(logits)에서 다음 토큰을 고르는 규칙 (greedy).
//!
//! OpenAI Whisper의 기본 greedy 디코딩과 같은 규칙을 씁니다.
//! - 첫 토큰에서는 공백(`blank_id`)과 끝(`eot`)을 고르지 않습니다 (`SuppressBlank`).
//! - 말소리가 아닌 기호 토큰(`non_speech_tokens`)은 고르지 않습니다 (`suppress_tokens="-1"`).
//! - 텍스트 토큰(`eot`보다 작은 ID)과 `eot`만 고릅니다. 언어·작업·타임스탬프 같은 특수 토큰은 나오지 않습니다.
//!
//! 온도를 올려 다시 디코딩하는 대비책(temperature fallback)은 없으므로, 같은 구절이 끝없이 되풀이되면
//! [`repetition_cut`]으로 끊습니다.

/// 다음 토큰 고르기 규칙
#[derive(Debug, Clone)]
pub struct TokenPicker {
    /// 고르지 않을 토큰 (`true`면 제외). 길이는 어휘 크기입니다.
    suppressed: Vec<bool>,
    eot: usize,
    blank: usize,
}

impl TokenPicker {
    pub fn new(vocab: usize, eot: i64, blank: i64, non_speech: &[i64]) -> Self {
        let eot = usize::try_from(eot).unwrap_or(0);
        let mut suppressed = vec![false; vocab];
        // eot보다 뒤는 모두 특수 토큰입니다.
        for flag in suppressed.iter_mut().skip(eot + 1) {
            *flag = true;
        }
        for &id in non_speech {
            if let Some(flag) = usize::try_from(id).ok().and_then(|i| suppressed.get_mut(i)) {
                *flag = true;
            }
        }
        Self {
            suppressed,
            eot,
            blank: usize::try_from(blank).unwrap_or(usize::MAX),
        }
    }

    /// 가장 점수가 높은 토큰. `first`면 공백과 끝을 고르지 않습니다.
    /// 고를 수 있는 토큰이 없으면(모두 NaN 등) `eot`입니다.
    pub fn pick(&self, logits: &[f32], first: bool) -> i64 {
        let mut best = None;
        let mut best_score = f32::NEG_INFINITY;
        for (id, &score) in logits.iter().enumerate() {
            if self.suppressed.get(id).copied().unwrap_or(true)
                || (first && (id == self.eot || id == self.blank))
            {
                continue;
            }
            if score > best_score {
                best_score = score;
                best = Some(id);
            }
        }
        best.unwrap_or(self.eot) as i64
    }

    pub fn eot(&self) -> i64 {
        self.eot as i64
    }
}

/// 언어 토큰 가운데 점수가 가장 높은 것의 위치. `languages`가 비었거나 범위를 벗어나면 `None`입니다.
pub fn best_language(logits: &[f32], languages: &[i64]) -> Option<usize> {
    languages
        .iter()
        .enumerate()
        .filter_map(|(index, &id)| {
            let score = *logits.get(usize::try_from(id).ok()?)?;
            (!score.is_nan()).then_some((index, score))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// 같은 구절이 이만큼 이어지면 되풀이로 봅니다. 짧은 구절일수록 더 많이 되풀이되어야 합니다.
const MIN_REPEATS: usize = 4;
const MIN_REPEATED_TOKENS: usize = 32;
const MAX_PATTERN: usize = 32;

/// 끝부분이 같은 구절의 되풀이면 남길 토큰 수(구절 하나만 남김)를 돌려줍니다.
///
/// 길이 `p`인 구절이 끝에서 `max(4, 32 / p)`번 이상 연달아 나오면 되풀이입니다
/// (예: 토큰 하나는 32번, 8개짜리 구절은 4번).
pub fn repetition_cut(tokens: &[i64]) -> Option<usize> {
    for pattern in 1..=MAX_PATTERN.min(tokens.len() / MIN_REPEATS) {
        let needed = MIN_REPEATS.max(MIN_REPEATED_TOKENS.div_ceil(pattern));
        if tokens.len() < pattern * needed {
            continue;
        }
        let tail = &tokens[tokens.len() - pattern..];
        let repeats = tokens
            .rchunks_exact(pattern)
            .take_while(|chunk| *chunk == tail)
            .count();
        if repeats >= needed {
            return Some(tokens.len() - (repeats - 1) * pattern);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logits(vocab: usize, high: &[(usize, f32)]) -> Vec<f32> {
        let mut out = vec![0.0; vocab];
        for &(id, score) in high {
            out[id] = score;
        }
        out
    }

    #[test]
    fn picks_best_text_token_and_skips_special_tokens() {
        // 어휘 20, eot 10, 공백 3, 기호 5
        let picker = TokenPicker::new(20, 10, 3, &[5]);
        assert_eq!(picker.pick(&logits(20, &[(7, 1.0), (2, 0.5)]), false), 7);
        // 특수 토큰(eot 뒤)과 기호 토큰은 점수가 높아도 고르지 않습니다.
        assert_eq!(
            picker.pick(&logits(20, &[(15, 9.0), (5, 8.0), (2, 1.0)]), false),
            2
        );
        // 끝 토큰은 첫 토큰이 아니면 고를 수 있습니다.
        assert_eq!(picker.pick(&logits(20, &[(10, 9.0), (2, 1.0)]), false), 10);
    }

    #[test]
    fn first_token_is_never_blank_or_end() {
        let picker = TokenPicker::new(20, 10, 3, &[]);
        let scores = logits(20, &[(10, 9.0), (3, 8.0), (4, 1.0)]);
        assert_eq!(picker.pick(&scores, true), 4);
        assert_eq!(picker.pick(&scores, false), 10);
    }

    #[test]
    fn nan_scores_fall_back_to_end() {
        let picker = TokenPicker::new(4, 2, 0, &[]);
        assert_eq!(picker.pick(&[f32::NAN; 4], false), 2);
        assert_eq!(picker.eot(), 2);
    }

    #[test]
    fn best_language_picks_highest_language_token() {
        let scores = logits(10, &[(6, 3.0), (8, 5.0), (2, 9.0)]);
        // 언어 토큰이 아닌 2는 점수가 높아도 보지 않습니다.
        assert_eq!(best_language(&scores, &[6, 8, 7]), Some(1));
        assert_eq!(best_language(&scores, &[]), None);
        assert_eq!(best_language(&scores, &[99]), None);
    }

    #[test]
    fn repetition_is_cut_to_one_copy() {
        // 3토큰 구절이 11번 (33토큰) 되풀이: 앞부분 + 구절 하나만 남깁니다.
        let mut tokens = vec![1, 2];
        for _ in 0..11 {
            tokens.extend([7, 8, 9]);
        }
        assert_eq!(repetition_cut(&tokens), Some(5));
        // 같은 토큰 32번
        let mut same = vec![4];
        same.extend([5; 32]);
        assert_eq!(repetition_cut(&same), Some(2));
    }

    #[test]
    fn short_or_varied_sequences_are_kept() {
        assert_eq!(repetition_cut(&[]), None);
        assert_eq!(repetition_cut(&[1, 2, 3, 4, 5, 6]), None);
        // 같은 토큰 31번은 아직 되풀이로 보지 않습니다 ("ㅋㅋㅋ" 같은 말).
        assert_eq!(repetition_cut(&[5; 31]), None);
        // 8토큰 문장이 세 번은 그대로 둡니다.
        let sentence = [10, 11, 12, 13, 14, 15, 16, 17];
        let three: Vec<i64> = sentence.iter().copied().cycle().take(24).collect();
        assert_eq!(repetition_cut(&three), None);
        let four: Vec<i64> = sentence.iter().copied().cycle().take(32).collect();
        assert_eq!(repetition_cut(&four), Some(8));
    }
}
