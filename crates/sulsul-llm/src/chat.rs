//! 글 다듬기 요청의 모양과 응답 정리.
//!
//! 지시문(고른 프롬프트)은 system 메시지로, 받아쓴 글은 user 메시지로 보냅니다. 외부 제공자는 두 메시지를
//! 그대로 받고([`crate::client::chat`]), 로컬 모델은 계열마다 정해진 채팅 형식으로 이어 붙인 글 한 덩어리를
//! 받습니다([`ChatFormat::render`]).
//!
//! 로컬 모델의 채팅 형식은 GGUF 안의 Jinja 템플릿을 해석하지 않고 계열별로 직접 씁니다. llama.cpp에
//! 들어 있는 간단한 템플릿 처리기는 Gemma 4 형식을 모르고, 카탈로그의 계열은 정해져 있기 때문입니다.
//! 각 형식은 공식 채팅 템플릿에 system·user 메시지 하나씩과 `add_generation_prompt`를 넣은 결과와 같습니다.
//! 맨 앞의 BOS 토큰은 넣지 않습니다. 토큰으로 바꿀 때 모델 설정에 따라 붙입니다.

/// 로컬 모델 계열의 채팅 형식
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatFormat {
    /// Qwen3.5 (ChatML). 생각(thinking) 없이 바로 답하도록 빈 생각 블록을 미리 넣습니다
    /// (공식 템플릿의 `enable_thinking=false`와 같음).
    Qwen,
    /// Gemma 4. 생각 모드는 system 차례에 `<|think|>`를 넣을 때만 켜지므로 넣지 않습니다.
    Gemma4,
    /// EXAONE 3.5
    Exaone,
}

impl ChatFormat {
    /// 카탈로그 계열 ID에 맞는 형식. 모르는 계열이면 `None`입니다.
    pub fn for_family(family_id: &str) -> Option<Self> {
        match family_id {
            "qwen3_5" => Some(Self::Qwen),
            "gemma4" => Some(Self::Gemma4),
            "exaone3_5" => Some(Self::Exaone),
            _ => None,
        }
    }

    /// 지시문과 받아쓴 글을 모델이 바로 답을 이어 쓸 수 있는 프롬프트로 이어 붙입니다.
    pub fn render(self, instructions: &str, text: &str) -> String {
        let instructions = instructions.trim();
        let text = text.trim();
        match self {
            Self::Qwen => format!(
                "<|im_start|>system\n{instructions}<|im_end|>\n\
                 <|im_start|>user\n{text}<|im_end|>\n\
                 <|im_start|>assistant\n<think>\n\n</think>\n\n"
            ),
            Self::Gemma4 => format!(
                "<|turn>system\n{instructions}<turn|>\n\
                 <|turn>user\n{text}<turn|>\n\
                 <|turn>model\n"
            ),
            Self::Exaone => format!(
                "[|system|]{instructions}[|endofturn|]\n\
                 [|user|]{text}\n\
                 [|assistant|]"
            ),
        }
    }
}

/// 생각 내용을 감싸는 표시 (여는 것, 닫는 것)
const THINKING_MARKS: [(&str, &str); 2] = [("<think>", "</think>"), ("<|channel>", "<channel|>")];

/// 모델 응답에서 다듬은 글만 남깁니다.
///
/// 생각하는 모델이 본문에 섞어 보내는 생각 블록(`<think>…</think>`, Gemma의 `<|channel>…<channel|>`)을
/// 지우고 앞뒤 공백을 지웁니다. 여는 표시 없이 닫는 표시만 있으면(프롬프트가 여는 표시를 넣은 경우)
/// 닫는 표시까지 지웁니다. 닫히지 않은 생각 블록은 답이 아직 나오지 않은 것이므로 통째로 지웁니다.
pub fn clean_output(raw: &str) -> String {
    let mut text = raw.to_owned();
    for (open, close) in THINKING_MARKS {
        text = strip_blocks(&text, open, close);
    }
    text.trim().to_owned()
}

fn strip_blocks(text: &str, open: &str, close: &str) -> String {
    let mut rest = text;
    // 여는 표시 없이 닫는 표시가 먼저 나오면 그 앞은 생각 내용입니다.
    if let Some(end) = rest.find(close)
        && rest.find(open).is_none_or(|start| end < start)
    {
        rest = &rest[end + close.len()..];
    }
    let mut kept = String::with_capacity(rest.len());
    while let Some(start) = rest.find(open) {
        kept.push_str(&rest[..start]);
        let after = &rest[start + open.len()..];
        match after.find(close) {
            Some(end) => rest = &after[end + close.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    kept.push_str(rest);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_families_have_a_format() {
        assert_eq!(ChatFormat::for_family("qwen3_5"), Some(ChatFormat::Qwen));
        assert_eq!(ChatFormat::for_family("gemma4"), Some(ChatFormat::Gemma4));
        assert_eq!(
            ChatFormat::for_family("exaone3_5"),
            Some(ChatFormat::Exaone)
        );
        assert_eq!(ChatFormat::for_family("llama"), None);
        for spec in crate::catalog::FAMILIES {
            assert!(
                ChatFormat::for_family(spec.id).is_some(),
                "카탈로그 계열 {}의 채팅 형식이 없습니다",
                spec.id
            );
        }
    }

    #[test]
    fn qwen_prompt_skips_thinking() {
        assert_eq!(
            ChatFormat::Qwen.render(" 다듬어 주세요. ", "\n안녕 하세요\n"),
            "<|im_start|>system\n다듬어 주세요.<|im_end|>\n\
             <|im_start|>user\n안녕 하세요<|im_end|>\n\
             <|im_start|>assistant\n<think>\n\n</think>\n\n"
        );
    }

    #[test]
    fn gemma_and_exaone_prompts_follow_their_templates() {
        assert_eq!(
            ChatFormat::Gemma4.render("지시", "글"),
            "<|turn>system\n지시<turn|>\n<|turn>user\n글<turn|>\n<|turn>model\n"
        );
        assert_eq!(
            ChatFormat::Exaone.render("지시", "글"),
            "[|system|]지시[|endofturn|]\n[|user|]글\n[|assistant|]"
        );
    }

    #[test]
    fn thinking_is_removed_from_the_output() {
        assert_eq!(clean_output("  안녕하세요.  "), "안녕하세요.");
        assert_eq!(
            clean_output("<think>\n맞춤법을 본다\n</think>\n\n안녕하세요."),
            "안녕하세요."
        );
        assert_eq!(
            clean_output("생각만 했다</think>안녕하세요."),
            "안녕하세요."
        );
        assert_eq!(
            clean_output("<|channel>thought\n<channel|>안녕하세요."),
            "안녕하세요."
        );
        assert_eq!(
            clean_output("앞<think>a</think>가운데<think>b</think>뒤"),
            "앞가운데뒤"
        );
        assert_eq!(clean_output("<think>끝나지 않은 생각"), "");
    }
}
