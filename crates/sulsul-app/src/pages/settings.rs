//! 설정 화면. 여기서 바꾼 값은 TOML 설정 파일에 저장됩니다.
//!
//! 음성 인식 엔진·모델·제공자와 그 설정(인식 언어, 용어 힌트, 엔진별 설정)은 모델 관리 화면에서
//! 고릅니다. 여기에는 두지 않습니다.

use egui::{DragValue, Ui};
use rust_i18n::t;
use sulsul_core::config::{
    Config, ConfigIssue, HotkeyMode, MAX_RECORD_SECONDS, OutputMode, RESTORE_DELAY_MS, Theme,
};
use sulsul_core::settings;
use sulsul_core::stt::SttRegistry;

use super::record::{hotkey_key_name, hotkey_mode_name};
use super::{SECTION_GAP, header};
use crate::error_text;
use crate::i18n;
use crate::icons::Icon;
use crate::session::PageContext;
use crate::system;
use crate::widgets::banner::{Banner, Tone};
use crate::widgets::card::Section;
use crate::widgets::confirm::{Answer, Confirm};
use crate::widgets::controls::{self, ComboItem};
use crate::widgets::device_combo::device_combo;
use crate::widgets::row::{self, CONTROL_WIDTH, Row};
use crate::widgets::text;

/// 단축키로 고를 수 있는 키. 화면에서 좌우 Ctrl을 구분해 입력받을 수 없어 목록에서 고릅니다.
const HOTKEY_PRESETS: [&str; 17] = [
    "RightCtrl",
    "RightAlt",
    "RightShift",
    "F13",
    "F14",
    "F15",
    "F16",
    "F17",
    "F18",
    "F19",
    "F20",
    "F21",
    "F22",
    "F23",
    "F24",
    "ScrollLock",
    "Pause",
];

#[derive(Default)]
pub struct SettingsPage {
    confirm_reset: bool,
}

impl SettingsPage {
    /// `issues`는 설정 파일을 읽으며 발견한 문제입니다.
    pub fn ui(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>, issues: &[ConfigIssue]) {
        header(ui, &t!("settings.title"), &t!("settings.subtitle"));

        if !issues.is_empty() {
            issues_banner(ui, issues);
            ui.add_space(SECTION_GAP);
        }

        self.file_card(ui, cx);

        // 설정 파일을 읽지 못하는 동안에는 편집을 잠급니다 (안내는 앱 위쪽 배너).
        ui.add_enabled_ui(!cx.locked, |ui| {
            let soon = t!("common.coming_soon");
            ui.add_space(SECTION_GAP);
            general_section(ui, cx.config, &soon);
            ui.add_space(SECTION_GAP);
            hotkey_section(ui, cx.config, &soon);
            ui.add_space(SECTION_GAP);
            audio_section(ui, cx, &soon);
            ui.add_space(SECTION_GAP);
            output_section(ui, cx.config, &soon);
            ui.add_space(SECTION_GAP);
            postprocess_section(ui, cx.config, &soon);
        });

        if self.confirm_reset {
            let answer = Confirm {
                id: "confirm_reset_settings",
                title: &t!("settings.reset.confirm_title"),
                body: &t!("settings.reset.confirm_body"),
                confirm_label: &t!("settings.reset.confirm"),
                cancel_label: &t!("common.cancel"),
                danger: true,
            }
            .show(ui);
            match answer {
                Some(Answer::Confirm) => {
                    self.confirm_reset = false;
                    if !cx.locked {
                        reset_to_defaults(cx.config, &cx.shared.registry);
                        cx.shared.toasts.info(t!("settings.reset.done"));
                    }
                }
                Some(Answer::Cancel) => self.confirm_reset = false,
                None => {}
            }
        }
    }

    fn file_card(&mut self, ui: &mut Ui, cx: &mut PageContext<'_>) {
        Section::new(Icon::FileCog, &t!("settings.file.title")).show(ui, |ui| {
            let path = cx.shared.paths.config_file();
            ui.add(text::path_label(ui, &path));
            ui.label(text::caption(ui, t!("settings.file.help")));
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if controls::icon_text_button(ui, Icon::FilePen, t!("settings.file.open")).clicked()
                    && let Err(message) = system::open_path(&path)
                {
                    cx.shared
                        .toasts
                        .error(t!("common.open_failed", message = message));
                }
                if controls::icon_text_button(ui, Icon::FolderOpen, t!("settings.file.open_folder"))
                    .clicked()
                    && let Err(message) = system::open_dir(&cx.shared.paths.config_dir)
                {
                    cx.shared
                        .toasts
                        .error(t!("common.open_failed", message = message));
                }
                ui.add_enabled_ui(!cx.locked, |ui| {
                    if controls::icon_text_button(ui, Icon::RotateCcw, t!("settings.reset.button"))
                        .clicked()
                    {
                        self.confirm_reset = true;
                    }
                });
            });
        });
    }
}

fn issues_banner(ui: &mut Ui, issues: &[ConfigIssue]) {
    Banner::new(Tone::Warning)
        .title(&t!("settings.issues.title", count = issues.len()))
        .lines(
            issues
                .iter()
                .map(|issue| format!("•  {}", error_text::config_issue(issue))),
        )
        .line(t!("settings.issues.help"))
        .show(ui);
}

fn general_section(ui: &mut Ui, config: &mut Config, soon: &str) {
    Section::new(Icon::SlidersHorizontal, &t!("settings.general.title")).show(ui, |ui| {
        let general = &mut config.general;

        let locales = i18n::available_locales();
        let items: Vec<_> = locales
            .iter()
            .map(|code| ComboItem::new(code.clone(), i18n::language_name(code)).detail(code))
            .collect();
        let missing = (!locales.contains(&general.ui_language)).then(|| {
            t!(
                "settings.general.language_missing",
                code = general.ui_language.as_str()
            )
            .into_owned()
        });
        Row::new(&t!("settings.general.language"))
            .warning_opt(missing.as_deref())
            .show(ui, |ui| {
                let fallback = general.ui_language.clone();
                controls::combo(
                    ui,
                    "ui_language",
                    CONTROL_WIDTH,
                    &mut general.ui_language,
                    &items,
                    &fallback,
                );
            });
        row::separator(ui);

        Row::new(&t!("settings.general.theme")).show(ui, |ui| {
            controls::segmented(
                ui,
                &mut general.theme,
                &[
                    (Theme::Dark, t!("settings.general.theme_dark").into_owned()),
                    (
                        Theme::Light,
                        t!("settings.general.theme_light").into_owned(),
                    ),
                    (
                        Theme::System,
                        t!("settings.general.theme_system").into_owned(),
                    ),
                ],
            );
        });
        row::separator(ui);

        let label = t!("settings.general.launch_at_startup");
        Row::new(&label).badge_opt(Some(soon)).show(ui, |ui| {
            controls::toggle(ui, &mut general.launch_at_startup, &label)
        });
        row::separator(ui);

        let label = t!("settings.general.start_minimized");
        Row::new(&label)
            .help(&t!("settings.general.start_minimized_help"))
            .badge_opt(Some(soon))
            .show(ui, |ui| {
                controls::toggle(ui, &mut general.start_minimized, &label)
            });
    });
}

fn hotkey_section(ui: &mut Ui, config: &mut Config, soon: &str) {
    Section::new(Icon::Keyboard, &t!("settings.hotkey.title"))
        .badge(soon)
        .show(ui, |ui| {
            let hotkey = &mut config.hotkey;

            // 설정 파일에 목록에 없는 키(조합 키 등)가 있으면 그 값도 보여 줍니다.
            let mut items: Vec<_> = HOTKEY_PRESETS
                .iter()
                .map(|key| ComboItem::new((*key).to_owned(), hotkey_key_name(key)))
                .collect();
            if !HOTKEY_PRESETS.contains(&hotkey.key.as_str()) {
                items.push(ComboItem::new(
                    hotkey.key.clone(),
                    hotkey_key_name(&hotkey.key),
                ));
            }
            Row::new(&t!("settings.hotkey.key"))
                .help(&t!("settings.hotkey.key_help"))
                .show(ui, |ui| {
                    let fallback = hotkey.key.clone();
                    controls::combo(
                        ui,
                        "hotkey_key",
                        CONTROL_WIDTH,
                        &mut hotkey.key,
                        &items,
                        &fallback,
                    );
                });
            row::separator(ui);

            let help = match hotkey.mode {
                HotkeyMode::Toggle => t!("settings.hotkey.mode_toggle_help"),
                HotkeyMode::PushToTalk => t!("settings.hotkey.mode_push_to_talk_help"),
            };
            Row::new(&t!("settings.hotkey.mode"))
                .help(&help)
                .show(ui, |ui| {
                    controls::segmented(
                        ui,
                        &mut hotkey.mode,
                        &[
                            (HotkeyMode::Toggle, hotkey_mode_name(HotkeyMode::Toggle)),
                            (
                                HotkeyMode::PushToTalk,
                                hotkey_mode_name(HotkeyMode::PushToTalk),
                            ),
                        ],
                    );
                });
        });
}

fn audio_section(ui: &mut Ui, cx: &mut PageContext<'_>, soon: &str) {
    Section::new(Icon::Mic, &t!("settings.audio.title")).show(ui, |ui| {
        let audio = &mut cx.config.audio;

        Row::new(&t!("settings.audio.input_device"))
            .help(&t!("settings.audio.input_device_help"))
            .show(ui, |ui| {
                device_combo(
                    ui,
                    "settings_device",
                    CONTROL_WIDTH,
                    &mut audio.input_device,
                    &cx.shared.devices,
                );
            });
        row::separator(ui);

        let (min, max) = MAX_RECORD_SECONDS;
        Row::new(&t!("settings.audio.max_record_seconds"))
            .help(&t!(
                "settings.audio.max_record_seconds_help",
                min = min,
                max = max
            ))
            .show(ui, |ui| {
                ui.add(
                    DragValue::new(&mut audio.max_record_seconds)
                        .range(min..=max)
                        // 범위를 벗어난 값이 파일에 있어도 사용자가 고치기 전에는 그대로 둡니다.
                        .clamp_existing_to_range(false)
                        .speed(1.0)
                        .suffix(t!("unit.seconds_suffix")),
                );
            });
        row::separator(ui);

        let label = t!("settings.audio.feedback_sounds");
        Row::new(&label)
            .help(&t!("settings.audio.feedback_sounds_help"))
            .badge_opt(Some(soon))
            .show(ui, |ui| {
                controls::toggle(ui, &mut audio.feedback_sounds, &label)
            });
    });
}

fn output_section(ui: &mut Ui, config: &mut Config, soon: &str) {
    Section::new(Icon::ClipboardPaste, &t!("settings.output.title"))
        .badge(soon)
        .show(ui, |ui| {
            let output = &mut config.output;

            let items = [
                ComboItem::new(OutputMode::Paste, t!("settings.output.mode_paste")),
                ComboItem::new(OutputMode::Type, t!("settings.output.mode_type")),
                ComboItem::new(OutputMode::Clipboard, t!("settings.output.mode_clipboard")),
            ];
            let help = match output.mode {
                OutputMode::Paste => t!("settings.output.mode_paste_help"),
                OutputMode::Type => t!("settings.output.mode_type_help"),
                OutputMode::Clipboard => t!("settings.output.mode_clipboard_help"),
            };
            Row::new(&t!("settings.output.mode"))
                .help(&help)
                .show(ui, |ui| {
                    controls::combo(
                        ui,
                        "output_mode",
                        CONTROL_WIDTH,
                        &mut output.mode,
                        &items,
                        "",
                    );
                });
            row::separator(ui);

            let label = t!("settings.output.restore_clipboard");
            Row::new(&label)
                .help(&t!("settings.output.restore_clipboard_help"))
                .show(ui, |ui| {
                    controls::toggle(ui, &mut output.restore_clipboard, &label)
                });
            row::separator(ui);

            let (min, max) = RESTORE_DELAY_MS;
            Row::new(&t!("settings.output.restore_delay"))
                .help(&t!(
                    "settings.output.restore_delay_help",
                    min = min,
                    max = max
                ))
                .show(ui, |ui| {
                    ui.add(
                        DragValue::new(&mut output.restore_delay_ms)
                            .range(min..=max)
                            .clamp_existing_to_range(false)
                            .speed(5.0)
                            .suffix(t!("unit.milliseconds_suffix")),
                    );
                });
        });
}

fn postprocess_section(ui: &mut Ui, config: &Config, soon: &str) {
    Section::new(Icon::Sparkles, &t!("settings.postprocess.title"))
        .badge(soon)
        .show(ui, |ui| {
            let label = t!("settings.postprocess.llm");
            Row::new(&label)
                .help(&t!("settings.postprocess.llm_help"))
                .show(ui, |ui| {
                    // 아직 후처리기가 없어 켤 수 없습니다. 파일의 값만 보여 줍니다.
                    let mut enabled = config.postprocess.enabled;
                    ui.add_enabled_ui(false, |ui| controls::toggle(ui, &mut enabled, &label));
                });
        });
}

/// 모든 값을 기본값으로 되돌립니다.
///
/// 저장은 기존 파일에 값을 합치는 방식이라 `[stt.engines.*]` 표 자체는 파일에 남습니다.
/// 그래서 표를 비우는 대신, 등록된 엔진의 항목을 스키마 기본값으로 덮어씁니다.
fn reset_to_defaults(config: &mut Config, registry: &SttRegistry) {
    let mut fresh = Config::default();
    for (id, table) in &config.stt.engines {
        let table = match registry.descriptor(id) {
            Some(descriptor) => settings::defaults(&descriptor.settings),
            // 어떤 값이 기본인지 알 수 없는 엔진은 그대로 둡니다.
            None => table.clone(),
        };
        fresh.stt.engines.insert(id.clone(), table);
    }
    // 등록해 둔 외부 제공자(API 키 포함)는 기본값으로 되돌려도 지우지 않습니다.
    fresh.stt.api_profile = config.stt.api_profile.clone();
    fresh.stt.api_profiles = config.stt.api_profiles.clone();
    fresh.postprocess.api_profile = config.postprocess.api_profile.clone();
    fresh.postprocess.api_profiles = config.postprocess.api_profiles.clone();
    fresh.postprocess.processors = config.postprocess.processors.clone();
    let engine = fresh.stt.engine.clone();
    registry.ensure_engine_defaults(&mut fresh.stt, &engine);
    *config = fresh;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use sulsul_core::stt::mock::{MOCK_ENGINE_ID, MockEngineFactory};
    use sulsul_core::toml::Value;

    #[test]
    fn reset_restores_defaults_and_engine_tables() {
        let mut registry = SttRegistry::new();
        registry.register(Arc::new(MockEngineFactory::new()));
        registry.register(sulsul_stt_whisper::factory());

        let mut config = Config::default();
        config.general.theme = Theme::Light;
        config.audio.max_record_seconds = 5;
        config.stt.engine = MOCK_ENGINE_ID.into();
        let table = config.stt.engines.entry(MOCK_ENGINE_ID.into()).or_default();
        table.insert("delay_ms".into(), Value::Integer(9000));
        let mut other = sulsul_core::toml::Table::new();
        other.insert("custom".into(), Value::Boolean(true));
        config.stt.engines.insert("someone_else".into(), other);
        // 두 갈래에 등록해 둔 외부 제공자
        let stt_profile = sulsul_stt_cloud::profile_from_preset(
            "openai",
            "stt-openai".into(),
            "받아쓰기용".into(),
        );
        config.stt.api_profile = stt_profile.id.clone();
        config.stt.api_profiles.push(stt_profile.clone());
        let llm_profile = sulsul_llm::presets::profile_from_preset(
            "openai",
            "llm-openai".into(),
            "다듬기용".into(),
        );
        config.postprocess.enabled = true;
        config.postprocess.processor = sulsul_llm::CLOUD_PROCESSOR_ID.into();
        config.postprocess.api_profile = llm_profile.id.clone();
        config.postprocess.api_profiles.push(llm_profile.clone());

        reset_to_defaults(&mut config, &registry);

        // 등록해 둔 제공자와 고른 제공자는 두 갈래 모두 남습니다.
        assert_eq!(config.stt.api_profile, stt_profile.id);
        assert_eq!(config.stt.api_profiles, [stt_profile]);
        assert_eq!(config.postprocess.api_profile, llm_profile.id);
        assert_eq!(config.postprocess.api_profiles, [llm_profile]);
        // 나머지 후처리 설정은 기본값으로 돌아갑니다.
        assert_eq!(
            config.postprocess.enabled,
            Config::default().postprocess.enabled
        );
        assert_eq!(
            config.postprocess.processor,
            Config::default().postprocess.processor
        );

        assert_eq!(config.general, Config::default().general);
        assert_eq!(config.audio, Config::default().audio);
        assert_eq!(config.stt.engine, "whisper_local");
        // 파일에 남을 표의 값도 기본값으로 돌아갑니다.
        assert_eq!(
            config.stt.engines[MOCK_ENGINE_ID]["delay_ms"],
            Value::Integer(300)
        );
        // 활성 엔진의 기본 항목이 채워집니다.
        assert!(config.stt.engines["whisper_local"].contains_key("model"));
        // 모르는 엔진의 표는 건드리지 않습니다.
        assert_eq!(
            config.stt.engines["someone_else"]["custom"],
            Value::Boolean(true)
        );
    }

    #[test]
    fn hotkey_presets_include_default_key() {
        assert!(HOTKEY_PRESETS.contains(&Config::default().hotkey.key.as_str()));
    }
}
