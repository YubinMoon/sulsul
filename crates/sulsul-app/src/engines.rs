//! 사용할 수 있는 음성 인식 엔진 등록.

use sulsul_core::stt::SttRegistry;

pub fn registry() -> SttRegistry {
    let mut registry = SttRegistry::new();
    registry.register(sulsul_stt_whisper::factory());
    registry.register(sulsul_stt_sensevoice::factory());
    registry.register(sulsul_stt_cloud::factory());
    // 가짜 엔진은 화면 흐름 확인용이라 개발 빌드에만 넣습니다.
    #[cfg(debug_assertions)]
    registry.register(std::sync::Arc::new(
        sulsul_core::stt::mock::MockEngineFactory::new(),
    ));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::tr_opt;
    use sulsul_core::settings::SettingKind;

    #[test]
    fn whisper_is_registered() {
        let reg = registry();
        assert!(reg.descriptor(sulsul_stt_whisper::ENGINE_ID).is_some());
        assert!(reg.descriptor(sulsul_stt_sensevoice::ENGINE_ID).is_some());
        assert!(reg.descriptor(sulsul_stt_cloud::ENGINE_ID).is_some());
    }

    /// 엔진 설명자가 가리키는 번역 키가 번역 파일에 모두 있어야 합니다.
    #[test]
    fn engine_descriptors_have_translations() {
        rust_i18n::set_locale("ko");
        let registry = registry();
        let mut keys = Vec::new();
        for descriptor in registry.descriptors() {
            keys.push(descriptor.name_key.clone());
            keys.push(descriptor.description_key.clone());
            for field in &descriptor.settings {
                keys.push(field.label_key.clone());
                keys.extend(field.help_key.clone());
                keys.extend(field.unavailable_key.clone());
                if let SettingKind::Choice { options } = &field.kind {
                    keys.extend(options.iter().filter_map(|o| o.label_key.clone()));
                    keys.extend(options.iter().filter_map(|o| o.unavailable_key.clone()));
                }
            }
        }
        let missing: Vec<_> = keys
            .into_iter()
            .filter(|key| tr_opt(key).is_none())
            .collect();
        assert!(missing.is_empty(), "번역이 없는 키: {missing:?}");
    }
}
