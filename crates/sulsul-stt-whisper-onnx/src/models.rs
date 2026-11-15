//! 내려받을 수 있는 Whisper ONNX 모델 목록.
//!
//! k2-fsa sherpa-onnx가 OpenAI Whisper를 내보낸 공개 저장소(`csukuangfj/sherpa-onnx-whisper-<크기>`)를 씁니다.
//! 크기마다 저장소가 따로 있고, 한 저장소에 인코더·디코더(원본 fp32와 int8 동적 양자화)와 토큰 파일이 있습니다.
//! 디코더는 길이가 고정된(448) 자기 어텐션 KV 캐시를 입출력으로 주고받아, 단계마다 텐서 모양이 거의 바뀌지 않습니다.
//!
//! 모델을 추가하려면 [`MODELS`]에 한 줄을 더합니다. 크기와 SHA-256은 고정한 커밋의 Hugging Face API
//! (`https://huggingface.co/api/models/<저장소>/tree/<커밋>`)에서 그 파일의 `size`와 `lfs.oid` 값을 그대로
//! 옮겨 적습니다. 토큰 파일(`*-tokens.txt`)은 LFS 파일이 아니라 `lfs.oid`가 없으므로 내려받아 SHA-256을 직접
//! 계산했습니다. 다국어 모델의 토큰 파일은 모든 크기에서 같은 파일(git blob `a4edf0b7…`)이고, 영어 전용 모델의
//! 토큰 파일도 모든 크기에서 같습니다(git blob `3480e077…`).
//!
//! 2 GB가 넘는 원본 모델은 가중치가 `.onnx` 옆의 외부 파일(`*.weights`)에 있습니다. ONNX Runtime이 `.onnx`와
//! 같은 폴더에서 그 이름으로 찾으므로 파일 이름을 바꾸지 않고 같은 폴더에 둡니다.
//!
//! 목록 순서가 내려받기 화면의 단계별 선택지 순서가 되므로 작은 모델부터 적고, 같은 크기 안에서는
//! 다국어 모델 바로 뒤에 영어 전용 모델을, 작은 양자화 파일(`int8`)을 원본(`fp32`)보다 먼저 둡니다.
//! 모델 ID에는 점을 쓰지 않습니다(번역 키 경로가 깨짐). 새 단계 값을 넣었다면 `locales/*.yml`의
//! `engine.whisper_onnx.choices.<축 ID>.<값 ID>` 문구도 추가합니다.

use std::path::{Path, PathBuf};

use sulsul_core::AppPaths;
use sulsul_core::stt::{
    LanguageSupport, ModelAsset, ModelAxis, ModelFile, ModelVariant, VariantChoice,
    WHISPER_LANGUAGES,
};

use crate::ENGINE_ID;

/// 추천 모델이자 기본 모델. 한국어 정확도와 속도의 균형이 가장 좋습니다.
///
/// 이 엔진은 디코더(토큰마다 한 번)가 시간의 대부분을 쓰는데, turbo는 디코더가 4층뿐이라(small 12층,
/// medium 24층) 토큰당 비용이 작으면서 정확도는 Large V3급입니다. RTX 3060에서 8초 한국어가 GPU 1.8초,
/// CPU 5.4초였고 53초 녹음을 거의 틀리지 않고 받아썼습니다.
pub const DEFAULT_MODEL: &str = "large-v3-turbo-int8";

/// 모델 폴더(`<데이터>/models`) 아래에서 이 엔진의 파일을 두는 폴더
pub const MODEL_SUBDIR: &str = "whisper_onnx";

/// 다국어 모델의 언어 단계 값
pub const MULTILINGUAL: &str = "multi";

/// 영어 전용(`*.en`) 모델의 언어 단계 값
pub const ENGLISH_ONLY: &str = "en";

/// 원본 가중치의 정밀도 단계 값
pub const FP32: &str = "fp32";

/// int8 동적 양자화 가중치의 정밀도 단계 값
pub const INT8: &str = "int8";

/// 내려받을 파일 하나 (고정한 커밋의 Hugging Face 파일)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteFile {
    pub file_name: &'static str,
    pub size_bytes: u64,
    /// 파일 전체의 SHA-256 (소문자 16진수 64자)
    pub sha256: &'static str,
}

/// 내려받을 수 있는 Whisper ONNX 모델 (인코더·디코더·토큰 파일 한 벌)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperOnnxModel {
    pub id: &'static str,
    /// 내려받기 화면의 크기 단계 값 (예: `large-v3-turbo`)
    pub size: &'static str,
    /// 내려받기 화면의 언어 단계 값 ([`MULTILINGUAL`] 또는 [`ENGLISH_ONLY`])
    pub language: &'static str,
    /// 내려받기 화면의 정밀도 단계 값 ([`INT8`] 또는 [`FP32`])
    pub precision: &'static str,
    /// Hugging Face 저장소 (예: `csukuangfj/sherpa-onnx-whisper-tiny`)
    pub repo: &'static str,
    /// 크기·해시를 확인한 커밋. `main`은 내용이 바뀔 수 있어 커밋에 고정합니다.
    pub revision: &'static str,
    pub encoder: RemoteFile,
    pub decoder: RemoteFile,
    pub tokens: RemoteFile,
    /// 2 GB가 넘어 따로 받는 외부 가중치 파일 (`.onnx`와 같은 폴더에 둡니다)
    pub external: &'static [RemoteFile],
    pub recommended: bool,
}

const fn file(file_name: &'static str, size_bytes: u64, sha256: &'static str) -> RemoteFile {
    RemoteFile {
        file_name,
        size_bytes,
        sha256,
    }
}

const MULTI_TOKENS_SIZE: u64 = 816_730;
const MULTI_TOKENS_SHA256: &str =
    "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126";
const EN_TOKENS_SIZE: u64 = 835_554;
const EN_TOKENS_SHA256: &str = "306cd27f03c1a714eca7108e03d66b7dc042abe8c258b44c199a7ed9838dd930";

const TINY: &str = "csukuangfj/sherpa-onnx-whisper-tiny";
const TINY_REV: &str = "65176e2deb88badc814a94058666cadccc29b61c";
const TINY_EN: &str = "csukuangfj/sherpa-onnx-whisper-tiny.en";
const TINY_EN_REV: &str = "d026532c022fa99fd789d6b32446a1df7b6bfc43";
const BASE: &str = "csukuangfj/sherpa-onnx-whisper-base";
const BASE_REV: &str = "bb53ee204431c90d314c1cc08d28d23e5b7927cc";
const BASE_EN: &str = "csukuangfj/sherpa-onnx-whisper-base.en";
const BASE_EN_REV: &str = "59eea950fc76df2453efb57e6c0fd334548e8ffe";
const SMALL: &str = "csukuangfj/sherpa-onnx-whisper-small";
const SMALL_REV: &str = "8f3c18b358db4d1f2fc1eae49d75cd20989e4309";
const SMALL_EN: &str = "csukuangfj/sherpa-onnx-whisper-small.en";
const SMALL_EN_REV: &str = "d9533f69affd85061aee349af7fea5cb2996dbbe";
const MEDIUM: &str = "csukuangfj/sherpa-onnx-whisper-medium";
const MEDIUM_REV: &str = "8c31d28503847560985df21f90e14f0c736e075e";
const MEDIUM_EN: &str = "csukuangfj/sherpa-onnx-whisper-medium.en";
const MEDIUM_EN_REV: &str = "251ab4521f354490e8f2c206fbd0b7f3f6b0a7ec";
const TURBO: &str = "csukuangfj/sherpa-onnx-whisper-turbo";
const TURBO_REV: &str = "2ca6ff69fc878651b770880507669577ac41c2ff";
const LARGE_V3: &str = "csukuangfj/sherpa-onnx-whisper-large-v3";
const LARGE_V3_REV: &str = "2a6507094dd6020d939d78e3f1834a1d06267fca";

const fn multi_tokens(file_name: &'static str) -> RemoteFile {
    file(file_name, MULTI_TOKENS_SIZE, MULTI_TOKENS_SHA256)
}

const fn en_tokens(file_name: &'static str) -> RemoteFile {
    file(file_name, EN_TOKENS_SIZE, EN_TOKENS_SHA256)
}

/// sherpa-onnx Whisper ONNX 모델 카탈로그
pub const MODELS: &[WhisperOnnxModel] = &[
    WhisperOnnxModel {
        id: "tiny-int8",
        size: "tiny",
        language: MULTILINGUAL,
        precision: INT8,
        repo: TINY,
        revision: TINY_REV,
        encoder: file(
            "tiny-encoder.int8.onnx",
            12_937_772,
            "d24fb083ae3b1041fc24e97971d60e280c9342201fbb67b0ab428a8b4a51a434",
        ),
        decoder: file(
            "tiny-decoder.int8.onnx",
            89_855_401,
            "d2fece8dd42771f1df975c6c0445770d0c292bf7547c2cae04a6c0cc57540925",
        ),
        tokens: multi_tokens("tiny-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "tiny",
        size: "tiny",
        language: MULTILINGUAL,
        precision: FP32,
        repo: TINY,
        revision: TINY_REV,
        encoder: file(
            "tiny-encoder.onnx",
            37_647_080,
            "42c1d4cbf889632ba21ab6f0d4064c80209755f265ce5cd630db4a6793e7089c",
        ),
        decoder: file(
            "tiny-decoder.onnx",
            114_505_801,
            "e144c07dc6b55cece24392811f2d934b97013811f5e677d1315d341a0a74a25d",
        ),
        tokens: multi_tokens("tiny-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "tiny-en-int8",
        size: "tiny",
        language: ENGLISH_ONLY,
        precision: INT8,
        repo: TINY_EN,
        revision: TINY_EN_REV,
        encoder: file(
            "tiny.en-encoder.int8.onnx",
            12_937_772,
            "0ce578b827c94a961aacb8fa14b02f096504b337e5c94be37c36238cbe3e8bc6",
        ),
        decoder: file(
            "tiny.en-decoder.int8.onnx",
            89_853_865,
            "06c0e6ff6348d427e51839219d1c886c18cfdf411e629e33f5e1679bff9c1527",
        ),
        tokens: en_tokens("tiny.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "tiny-en",
        size: "tiny",
        language: ENGLISH_ONLY,
        precision: FP32,
        repo: TINY_EN,
        revision: TINY_EN_REV,
        encoder: file(
            "tiny.en-encoder.onnx",
            37_647_080,
            "dc7de696432d97a3d64387800b230ac69d18e5e4efea0eec0613209dd8b7b0c9",
        ),
        decoder: file(
            "tiny.en-decoder.onnx",
            114_504_265,
            "54d5a5fd2a757175aeb70a837a8404fc9c31b659610724694f5a0e2c51715f94",
        ),
        tokens: en_tokens("tiny.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "base-int8",
        size: "base",
        language: MULTILINGUAL,
        precision: INT8,
        repo: BASE,
        revision: BASE_REV,
        encoder: file(
            "base-encoder.int8.onnx",
            29_120_534,
            "0b8fb1304b6109976038efff5ace81720e00386f3ff6b54ee8c75291ca0a1e11",
        ),
        decoder: file(
            "base-decoder.int8.onnx",
            130_672_026,
            "9759d217388a01b3a4c7c15533201067b48ae819c4daafc8624e64b9409dc02d",
        ),
        tokens: multi_tokens("base-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "base",
        size: "base",
        language: MULTILINGUAL,
        precision: FP32,
        repo: BASE,
        revision: BASE_REV,
        encoder: file(
            "base-encoder.onnx",
            95_087_154,
            "5a6b87cb313993f6c9fefec9e7027556f6cb30becddf49655bee36c50ecc12d7",
        ),
        decoder: file(
            "base-decoder.onnx",
            196_548_998,
            "8a12c3f6ad65bb5b86d7e6eccc302378f20f9fb2df6cb10747c62895da7ac194",
        ),
        tokens: multi_tokens("base-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "base-en-int8",
        size: "base",
        language: ENGLISH_ONLY,
        precision: INT8,
        repo: BASE_EN,
        revision: BASE_EN_REV,
        encoder: file(
            "base.en-encoder.int8.onnx",
            29_120_534,
            "ef6b936f4c9b1d90a3b68634b60c4ed8576b26172b33c2535ec0e933c9edb823",
        ),
        decoder: file(
            "base.en-decoder.int8.onnx",
            130_669_978,
            "f7162ad6db2dbef16cfaeaa7f945b9d7dd9c1b8d472f6aca82f2273d185e4d41",
        ),
        tokens: en_tokens("base.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "base-en",
        size: "base",
        language: ENGLISH_ONLY,
        precision: FP32,
        repo: BASE_EN,
        revision: BASE_EN_REV,
        encoder: file(
            "base.en-encoder.onnx",
            95_087_154,
            "f7eebd67fd79ede97c822439a3d88035ec26ac573db5e8a2a5dd45027e39c22c",
        ),
        decoder: file(
            "base.en-decoder.onnx",
            196_546_950,
            "4f49d5d76f95e6f6da717e3d50e271f21fd2ecac10b7dc566ba6b14924c68792",
        ),
        tokens: en_tokens("base.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "small-int8",
        size: "small",
        language: MULTILINGUAL,
        precision: INT8,
        repo: SMALL,
        revision: SMALL_REV,
        encoder: file(
            "small-encoder.int8.onnx",
            112_442_483,
            "4cbe7b22fa9026b843b60a68640c747de05bafb1a11b57edc0e66c232d9f33a9",
        ),
        decoder: file(
            "small-decoder.int8.onnx",
            262_226_114,
            "acad50b5c782696e91b55914cc5ab4f756f1532f76e22aa6fc615f39fb69a8ee",
        ),
        tokens: multi_tokens("small-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "small",
        size: "small",
        language: MULTILINGUAL,
        precision: FP32,
        repo: SMALL,
        revision: SMALL_REV,
        encoder: file(
            "small-encoder.onnx",
            409_528_992,
            "119bd1e8ba0524baee1687f6b22bf0abd2fe539549cd000734edbca81c66751e",
        ),
        decoder: file(
            "small-decoder.onnx",
            559_127_829,
            "a4165cca5c77e381938c0e111032a384901b1e434ae2ad948859035392d21d2c",
        ),
        tokens: multi_tokens("small-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "small-en-int8",
        size: "small",
        language: ENGLISH_ONLY,
        precision: INT8,
        repo: SMALL_EN,
        revision: SMALL_EN_REV,
        encoder: file(
            "small.en-encoder.int8.onnx",
            112_442_483,
            "8bdac288f369aa94ee2194059238c465ed82ea9d47ee8fa4a8c0a891873e462f",
        ),
        decoder: file(
            "small.en-decoder.int8.onnx",
            262_223_042,
            "710ccf890e10f3faa15f51ec346081a2723c9f3adb6e4da81c6573a5a6f877fb",
        ),
        tokens: en_tokens("small.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "small-en",
        size: "small",
        language: ENGLISH_ONLY,
        precision: FP32,
        repo: SMALL_EN,
        revision: SMALL_EN_REV,
        encoder: file(
            "small.en-encoder.onnx",
            409_528_992,
            "e6e3d58bc6600d54f18b982a800c3cb56c62b014da127ef4adb175e6d28f2702",
        ),
        decoder: file(
            "small.en-decoder.onnx",
            559_124_757,
            "619aa554c30f6440d1c087e72695a65b2d9cfdfb415ef7152d652c2685ee4cc3",
        ),
        tokens: en_tokens("small.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "medium-int8",
        size: "medium",
        language: MULTILINGUAL,
        precision: INT8,
        repo: MEDIUM,
        revision: MEDIUM_REV,
        encoder: file(
            "medium-encoder.int8.onnx",
            374_196_283,
            "1c54582b4d829de0089f6cb63bbbdb3bf7555398bacaf855fbecf1a84dfd193e",
        ),
        decoder: file(
            "medium-decoder.int8.onnx",
            571_059_257,
            "595d00a338a365a7bfa0ca7f296cabc639583bef770ab6130df90f49a6412747",
        ),
        tokens: multi_tokens("medium-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "medium",
        size: "medium",
        language: MULTILINGUAL,
        precision: FP32,
        repo: MEDIUM,
        revision: MEDIUM_REV,
        encoder: file(
            "medium-encoder.onnx",
            1_430_791_931,
            "352c45af645ce03ef83dbb68f63e5cea13baa5537e5feb13bc88a5976df6a4fd",
        ),
        decoder: file(
            "medium-decoder.onnx",
            1_627_278_264,
            "c3c7b60335a3564f265162846d2a815b3f020674daeab66107b58618077dd547",
        ),
        tokens: multi_tokens("medium-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "medium-en-int8",
        size: "medium",
        language: ENGLISH_ONLY,
        precision: INT8,
        repo: MEDIUM_EN,
        revision: MEDIUM_EN_REV,
        encoder: file(
            "medium.en-encoder.int8.onnx",
            374_196_283,
            "5a8e3a36619e0b67db9320eef3152db59d4b440f5ce0212d2c162a61b750bf80",
        ),
        decoder: file(
            "medium.en-decoder.int8.onnx",
            571_055_161,
            "7303be339ed4e51f4ffb7ae84f3803b10cf8e67e1dcf8a98cb4d843f0dea0141",
        ),
        tokens: en_tokens("medium.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "medium-en",
        size: "medium",
        language: ENGLISH_ONLY,
        precision: FP32,
        repo: MEDIUM_EN,
        revision: MEDIUM_EN_REV,
        encoder: file(
            "medium.en-encoder.onnx",
            1_430_791_931,
            "c29fc4804b9d9f62b52554e34c265eb49091620b67804b3eec6baec967dc7b94",
        ),
        decoder: file(
            "medium.en-decoder.onnx",
            1_627_274_168,
            "7573282efc94a81a73923d090da0a026da3aa85a1a22f2a5fe4a88808575bc8d",
        ),
        tokens: en_tokens("medium.en-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "large-v3-turbo-int8",
        size: "large-v3-turbo",
        language: MULTILINGUAL,
        precision: INT8,
        repo: TURBO,
        revision: TURBO_REV,
        encoder: file(
            "turbo-encoder.int8.onnx",
            674_716_297,
            "b02dcdf54f348741e93fe732b67d933c8dcb6735655f710640143081db38878b",
        ),
        decoder: file(
            "turbo-decoder.int8.onnx",
            361_080_764,
            "20accd02388482eb3a46bd615631adfdc85e1eb2c7db9ea3f02a40ffe6b81547",
        ),
        tokens: multi_tokens("turbo-tokens.txt"),
        external: &[],
        recommended: true,
    },
    WhisperOnnxModel {
        id: "large-v3-turbo",
        size: "large-v3-turbo",
        language: MULTILINGUAL,
        precision: FP32,
        repo: TURBO,
        revision: TURBO_REV,
        encoder: file(
            "turbo-encoder.onnx",
            735_920,
            "1b960f278564fb8bbacd544d4f85f4dd6d8a64d3aa89543d8f2c4021c926f976",
        ),
        decoder: file(
            "turbo-decoder.onnx",
            636_209_532,
            "b24db5d90fa230c5eaa6b823d74862ced9e0d1dc3e01ec46601968e8db0e09ec",
        ),
        tokens: multi_tokens("turbo-tokens.txt"),
        external: &[file(
            "turbo-encoder.weights",
            2_600_325_120,
            "746f879ecf066450ab0cdecc05383380b85157270ff6c0a9fb7cfdd917036e12",
        )],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "large-v3-int8",
        size: "large-v3",
        language: MULTILINGUAL,
        precision: INT8,
        repo: LARGE_V3,
        revision: LARGE_V3_REV,
        encoder: file(
            "large-v3-encoder.int8.onnx",
            766_671_985,
            "d531cf17248acc43e8c09b472a0877055e770877857a5332fc1304b36534ec85",
        ),
        decoder: file(
            "large-v3-decoder.int8.onnx",
            1_008_265_203,
            "ebc6bfd88e162a46cb3edee8a7e727e1dcbc65cabecb19e2573695e4d495e1af",
        ),
        tokens: multi_tokens("large-v3-tokens.txt"),
        external: &[],
        recommended: false,
    },
    WhisperOnnxModel {
        id: "large-v3",
        size: "large-v3",
        language: MULTILINGUAL,
        precision: FP32,
        repo: LARGE_V3,
        revision: LARGE_V3_REV,
        encoder: file(
            "large-v3-encoder.onnx",
            762_135,
            "a55051fb0570a1a1c1d05b57988b949bf029fee2bd218245250720353076b756",
        ),
        decoder: file(
            "large-v3-decoder.onnx",
            2_835_288,
            "fc6e718cd7863e5d7330fb581347cb397dee756a0ed2063990f37ca20a4ad666",
        ),
        tokens: multi_tokens("large-v3-tokens.txt"),
        external: &[
            file(
                "large-v3-encoder.weights",
                2_967_470_080,
                "23ed0376b33fd0e2b9de5043ca2830ce5cdb0219ef18f79ced25987fe38e1cb4",
            ),
            file(
                "large-v3-decoder.weights",
                3_206_492_160,
                "b66bdf300b57c68ef7b31b2e4038fcd1377a105928af1ef621afd4dce7aa6b49",
            ),
        ],
        recommended: false,
    },
];

pub fn find_model(id: &str) -> Option<&'static WhisperOnnxModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// 카탈로그 모델을 내려받는 폴더 (`<데이터>/models/whisper_onnx`)
pub fn default_model_dir(paths: &AppPaths) -> PathBuf {
    paths.models_dir().join(MODEL_SUBDIR)
}

/// 다국어 모델이 인식할 수 있는 언어
pub fn multilingual() -> LanguageSupport {
    LanguageSupport::AutoAnd(WHISPER_LANGUAGES.iter().map(|&c| c.to_owned()).collect())
}

/// 인코더·디코더·토큰 파일 경로 한 벌
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPaths {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub tokens: PathBuf,
}

impl WhisperOnnxModel {
    /// 고정한 커밋의 내려받기 주소
    pub fn url(&self, file: &RemoteFile) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, file.file_name
        )
    }

    /// 필요한 모든 파일. 첫 번째가 주 모델 파일(인코더)이고, 그다음이 디코더, 토큰, 외부 가중치입니다.
    pub fn files(&self) -> impl Iterator<Item = &RemoteFile> {
        [&self.encoder, &self.decoder, &self.tokens]
            .into_iter()
            .chain(self.external)
    }

    pub fn total_bytes(&self) -> u64 {
        self.files().map(|f| f.size_bytes).sum()
    }

    /// 화면에 보일 전체 크기 (예: `1.0 GB`)
    pub fn size_label(&self) -> String {
        format_size(self.total_bytes())
    }

    /// `dir` 안에서 이 모델의 파일 경로
    pub fn paths_in(&self, dir: &Path) -> ModelPaths {
        ModelPaths {
            encoder: dir.join(self.encoder.file_name),
            decoder: dir.join(self.decoder.file_name),
            tokens: dir.join(self.tokens.file_name),
        }
    }

    /// 영어 전용(`*.en`) 모델인지
    pub fn is_english_only(&self) -> bool {
        self.language == ENGLISH_ONLY
    }

    /// 이 모델로 인식할 수 있는 언어. 영어 전용 모델은 영어만, 다국어 모델은 자동 감지와 Whisper의 모든 언어입니다.
    pub fn languages(&self) -> LanguageSupport {
        if self.is_english_only() {
            LanguageSupport::Only(vec![ENGLISH_ONLY.to_owned()])
        } else {
            multilingual()
        }
    }

    /// 내려받기 화면의 단계별 선택 값 (크기 → 언어 → 정밀도 순서)
    pub fn choices(&self) -> Vec<VariantChoice> {
        [
            (ModelAxis::Size, self.size),
            (ModelAxis::Language, self.language),
            (ModelAxis::Precision, self.precision),
        ]
        .into_iter()
        .map(|(axis, value)| VariantChoice::new(ENGINE_ID, axis, value))
        .collect()
    }

    /// 파일 하나의 내려받기 정보. 대표 파일(인코더)의 ID는 모델 ID이고, 나머지는 파일 이름입니다
    /// (다른 로컬 엔진과 같은 규칙. CLI `models` 명령이 이 ID를 모델 이름으로 보여 줍니다).
    fn asset(&self, file: &RemoteFile) -> ModelAsset {
        let id = if file.file_name == self.encoder.file_name {
            self.id
        } else {
            file.file_name
        };
        ModelAsset {
            id: id.to_owned(),
            file_name: file.file_name.to_owned(),
            url: self.url(file),
            size_bytes: file.size_bytes,
            sha256: file.sha256.to_owned(),
        }
    }

    /// 내려받기 화면과 [`sulsul_core::stt::ModelRequirement`]에 쓰는 공통 형식. 파일은 모두 `dir`에 둡니다.
    pub fn to_catalog_variant(&self, dir: &Path) -> ModelVariant {
        ModelVariant {
            id: self.id.to_owned(),
            name_key: format!("engine.{ENGINE_ID}.models.{}.name", self.id),
            description_key: format!("engine.{ENGINE_ID}.models.{}.description", self.id),
            recommended: self.recommended,
            files: self
                .files()
                .map(|file| ModelFile {
                    path: dir.join(file.file_name),
                    asset: self.asset(file),
                })
                .collect(),
            choices: self.choices(),
        }
    }
}

/// 바이트 수를 사람이 읽기 쉬운 단위로 바꿉니다 (1024 단위, 다른 로컬 엔진과 같은 기준).
fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value >= KIB * KIB * KIB {
        format!("{:.1} GB", value / (KIB * KIB * KIB))
    } else if value >= KIB * KIB {
        format!("{:.0} MB", value / (KIB * KIB))
    } else {
        format!("{:.0} KB", (value / KIB).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use sulsul_core::stt::{axis_options, catalog_axes, find_by_choices};

    fn is_hex(text: &str) -> bool {
        text.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    fn catalog() -> Vec<ModelVariant> {
        let dir = Path::new("models/whisper_onnx");
        MODELS.iter().map(|m| m.to_catalog_variant(dir)).collect()
    }

    fn picked(variant: &ModelVariant) -> Vec<(ModelAxis, &str)> {
        variant
            .choices
            .iter()
            .map(|c| (c.axis, c.value.as_str()))
            .collect()
    }

    #[test]
    fn default_model_is_listed_and_recommended() {
        let default = find_model(DEFAULT_MODEL).expect("기본 모델이 목록에 없습니다");
        assert!(default.recommended);
        let recommended: Vec<_> = MODELS
            .iter()
            .filter(|m| m.recommended)
            .map(|m| m.id)
            .collect();
        assert_eq!(recommended, [DEFAULT_MODEL], "추천 모델은 하나여야 합니다");
    }

    #[test]
    fn entries_are_well_formed() {
        for model in MODELS {
            assert_eq!(model.revision.len(), 40, "{}: 커밋 해시 길이", model.id);
            assert!(is_hex(model.revision), "{}: 커밋 해시", model.id);
            assert!(
                model.repo.starts_with("csukuangfj/sherpa-onnx-whisper-"),
                "{}: 한 계열 저장소만 씁니다",
                model.id
            );
            for file in model.files() {
                assert!(file.size_bytes > 0, "{}: {}", model.id, file.file_name);
                assert_eq!(file.sha256.len(), 64, "{}: {}", model.id, file.file_name);
                assert!(is_hex(file.sha256), "{}: {}", model.id, file.file_name);
                let url = model.url(file);
                assert!(
                    url.contains(model.revision) && !url.contains("/resolve/main/"),
                    "{}: 주소가 커밋에 고정되지 않았습니다",
                    model.id
                );
            }
            let names: HashSet<_> = model.files().map(|f| f.file_name).collect();
            assert_eq!(
                names.len(),
                model.files().count(),
                "{}: 파일 이름이 겹칩니다",
                model.id
            );
        }
    }

    #[test]
    fn model_ids_have_no_dots_and_are_unique() {
        let mut seen = HashSet::new();
        for model in MODELS {
            assert!(
                !model.id.contains('.'),
                "{}: 모델 ID에 점이 있으면 번역 키 경로가 깨집니다",
                model.id
            );
            assert!(seen.insert(model.id), "{}: 모델 ID가 겹칩니다", model.id);
        }
    }

    #[test]
    fn file_names_match_axes() {
        for model in MODELS {
            let int8 = model.precision == INT8;
            assert_eq!(
                model.encoder.file_name.ends_with(".int8.onnx"),
                int8,
                "{}: 인코더 파일과 정밀도가 맞지 않습니다",
                model.id
            );
            assert_eq!(
                model.decoder.file_name.ends_with(".int8.onnx"),
                int8,
                "{}: 디코더 파일과 정밀도가 맞지 않습니다",
                model.id
            );
            assert_eq!(
                model.repo.ends_with(".en"),
                model.is_english_only(),
                "{}: 저장소와 언어 단계가 맞지 않습니다",
                model.id
            );
            assert_eq!(
                model.tokens.sha256 == EN_TOKENS_SHA256,
                model.is_english_only(),
                "{}: 토큰 파일과 언어 단계가 맞지 않습니다",
                model.id
            );
            // 외부 가중치는 원본(fp32) 큰 모델에만 있고, 이름이 같은 폴더의 `.onnx`와 짝을 이룹니다.
            for external in model.external {
                assert_eq!(model.precision, FP32, "{}", model.id);
                let stem = external.file_name.strip_suffix(".weights").unwrap();
                assert!(
                    [model.encoder.file_name, model.decoder.file_name]
                        .contains(&format!("{stem}.onnx").as_str()),
                    "{}: {}의 짝 .onnx가 없습니다",
                    model.id,
                    external.file_name
                );
            }
        }
    }

    #[test]
    fn catalog_variant_lists_every_file_in_one_folder() {
        let dir = Path::new("models/whisper_onnx");
        let variant = find_model("large-v3").unwrap().to_catalog_variant(dir);
        let names: Vec<_> = variant
            .files
            .iter()
            .map(|f| f.path.strip_prefix(dir).unwrap().to_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "large-v3-encoder.onnx",
                "large-v3-decoder.onnx",
                "large-v3-tokens.txt",
                "large-v3-encoder.weights",
                "large-v3-decoder.weights",
            ]
        );
        assert_eq!(variant.primary_path(), Some(dir.join(names[0]).as_path()));
        // 대표 파일(인코더)의 ID는 모델 ID, 나머지는 파일 이름입니다.
        let ids: Vec<_> = variant.files.iter().map(|f| f.asset.id.as_str()).collect();
        assert_eq!(ids[0], "large-v3");
        assert_eq!(&ids[1..], &names[1..]);
        assert_eq!(
            variant.total_bytes(),
            find_model("large-v3").unwrap().total_bytes()
        );
        // 같은 크기의 int8·원본은 토큰 파일을 함께 씁니다 (지울 때 core가 공유 파일을 남깁니다).
        let tiny = find_model("tiny").unwrap().paths_in(dir);
        let tiny_int8 = find_model("tiny-int8").unwrap().paths_in(dir);
        assert_eq!(tiny.tokens, tiny_int8.tokens);
        assert_ne!(tiny.encoder, tiny_int8.encoder);
    }

    #[test]
    fn every_variant_has_same_axes_in_order() {
        let catalog = catalog();
        assert_eq!(
            catalog_axes(&catalog),
            [ModelAxis::Size, ModelAxis::Language, ModelAxis::Precision]
        );
        for variant in &catalog {
            let axes: Vec<_> = variant.choices.iter().map(|c| c.axis).collect();
            assert_eq!(
                axes,
                catalog_axes(&catalog),
                "{}: 단계 순서가 다른 변형과 다릅니다",
                variant.id
            );
        }
    }

    #[test]
    fn choice_combinations_are_unique_and_findable() {
        let catalog = catalog();
        let mut seen = HashSet::new();
        for variant in &catalog {
            let picks = picked(variant);
            assert!(
                seen.insert(picks.clone()),
                "{}: 같은 (크기, 언어, 정밀도) 조합을 가진 변형이 또 있습니다",
                variant.id
            );
            let found = find_by_choices(&catalog, &picks).unwrap();
            assert_eq!(
                found.id, variant.id,
                "단계 값으로 찾은 변형이 {}가 아닙니다",
                variant.id
            );
        }
    }

    #[test]
    fn choice_label_keys_follow_rule() {
        for variant in catalog() {
            for choice in &variant.choices {
                assert_eq!(
                    choice.label_key,
                    format!(
                        "engine.whisper_onnx.choices.{}.{}",
                        choice.axis.id(),
                        choice.value
                    ),
                    "{}: 번역 키 규칙이 다릅니다",
                    variant.id
                );
            }
        }
    }

    #[test]
    fn choice_values_match_model_ids() {
        let expected = [
            ("tiny-int8", "tiny", "multi", "int8"),
            ("tiny", "tiny", "multi", "fp32"),
            ("tiny-en-int8", "tiny", "en", "int8"),
            ("tiny-en", "tiny", "en", "fp32"),
            ("base-int8", "base", "multi", "int8"),
            ("base", "base", "multi", "fp32"),
            ("base-en-int8", "base", "en", "int8"),
            ("base-en", "base", "en", "fp32"),
            ("small-int8", "small", "multi", "int8"),
            ("small", "small", "multi", "fp32"),
            ("small-en-int8", "small", "en", "int8"),
            ("small-en", "small", "en", "fp32"),
            ("medium-int8", "medium", "multi", "int8"),
            ("medium", "medium", "multi", "fp32"),
            ("medium-en-int8", "medium", "en", "int8"),
            ("medium-en", "medium", "en", "fp32"),
            ("large-v3-turbo-int8", "large-v3-turbo", "multi", "int8"),
            ("large-v3-turbo", "large-v3-turbo", "multi", "fp32"),
            ("large-v3-int8", "large-v3", "multi", "int8"),
            ("large-v3", "large-v3", "multi", "fp32"),
        ];
        let catalog = catalog();
        assert_eq!(catalog.len(), expected.len());
        for (variant, (id, size, language, precision)) in catalog.iter().zip(expected) {
            assert_eq!(variant.id, id, "모델 목록 순서가 다릅니다");
            let value = |axis| variant.choice(axis).unwrap().value.as_str();
            assert_eq!(value(ModelAxis::Size), size, "{id}: 크기 값");
            assert_eq!(value(ModelAxis::Language), language, "{id}: 언어 값");
            assert_eq!(value(ModelAxis::Precision), precision, "{id}: 정밀도 값");
        }
    }

    #[test]
    fn options_are_ordered_small_to_large_multilingual_and_int8_first() {
        let catalog = catalog();
        let values = |picked: &[(ModelAxis, &str)], axis| -> Vec<String> {
            axis_options(&catalog, picked, axis)
                .into_iter()
                .map(|c| c.value.clone())
                .collect()
        };
        assert_eq!(
            values(&[], ModelAxis::Size),
            [
                "tiny",
                "base",
                "small",
                "medium",
                "large-v3-turbo",
                "large-v3"
            ],
            "크기 선택지 순서가 작은 모델부터가 아닙니다"
        );
        for size in ["tiny", "base", "small", "medium"] {
            let picked = [(ModelAxis::Size, size)];
            assert_eq!(
                values(&picked, ModelAxis::Language),
                ["multi", "en"],
                "{size}"
            );
        }
        for size in ["large-v3-turbo", "large-v3"] {
            let picked = [(ModelAxis::Size, size)];
            assert_eq!(values(&picked, ModelAxis::Language), ["multi"], "{size}");
        }
        for size in ["tiny", "medium", "large-v3-turbo", "large-v3"] {
            let picked = [(ModelAxis::Size, size), (ModelAxis::Language, "multi")];
            assert_eq!(
                values(&picked, ModelAxis::Precision),
                ["int8", "fp32"],
                "{size}"
            );
        }
    }

    #[test]
    fn english_only_models_support_only_english() {
        for model in MODELS {
            let expected = if model.is_english_only() {
                LanguageSupport::Only(vec!["en".into()])
            } else {
                multilingual()
            };
            assert_eq!(model.languages(), expected, "{}", model.id);
        }
    }

    #[test]
    fn size_labels() {
        assert_eq!(find_model("large-v3").unwrap().size_label(), "5.8 GB");
        assert_eq!(find_model(DEFAULT_MODEL).unwrap().size_label(), "989 MB");
        assert_eq!(find_model("tiny-int8").unwrap().size_label(), "99 MB");
        assert_eq!(format_size(512), "1 KB");
    }
}
