---
name: sulsul-orchestration
description: >-
  술술(sulsul) 저장소에서 Orca `orchestration` 스킬로 워커를 띄울 때 그 위에 얹는 확장 스킬.
  orchestration의 절차는 그대로 쓰고, 작업 종류별로 어떤 워커(agent·model·effort)에게 맡길지,
  병렬로 나누는 법, 워커 spec에 넣을 내용, computer use 실행 테스트 방법을 정한다.
  이 저장소에서 /orchestration, Orca 워커, 병렬 워커, "워커에게 맡겨", "Codex한테 테스트시켜",
  GUI 작업 위임, 앱 실행 테스트를 언급하거나 orchestration 스킬을 쓰려는 순간에는
  orchestration만 쓰지 말고 반드시 이 스킬도 함께 불러라.
---

# 술술 오케스트레이션

이 스킬은 `orchestration` 스킬을 이어받아 술술 저장소용 규칙을 덧붙입니다.
`worker-start`, 턴 끝내기, 깨어났을 때 처리, `send`/`reply`, 복구 같은 **절차는 전부 orchestration을 따르고**,
여기서는 **누구에게 무엇을 어떤 모델로 맡길지**만 정합니다. 둘이 어긋나면 이 스킬이 우선합니다.

## 1. 먼저 orchestration 불러오기

1. `orchestration` 스킬을 불러 그 안내대로 `orca skills get orchestration`으로 Orca 버전에 맞는 가이드를 읽습니다.
   이 세션에서 이미 읽었다면 다시 읽지 않습니다.
2. 할 일을 아래 세 종류로 나누고, 워커마다 표의 옵션을 붙여 `worker-start` 합니다.

## 2. 워커 배정

| 종류                    | `--agent` | `--model`          | `--effort` | 띄우는 수                  |
| --------------------- | --------- | ------------------ | ---------- | ---------------------- |
| 일반 코드                 | `claude`  | `claude-opus-5-5`  | `xhigh`    | 서로 안 겹치는 조각마다 하나씩, 병렬  |
| GUI                   | `claude`  | `claude-fable-5-2` | `xhigh`    | 보통 하나 (화면이 완전히 나뉘면 여럿) |
| 실행 테스트 (computer use) | `codex`   | `gpt-6.1-terra`    | `high`     | 하나                     |

orchestration 가이드에는 "사용자가 모델을 말했을 때만 `--model`을 붙인다"고 되어 있지만, 이 표가 사용자가 정한
모델이므로 **항상 `--model`과 `--effort`를 붙입니다**. 빼면 각 에이전트의 기본 설정 모델로 떠서 배정이 무너집니다.
사용자가 어떤 작업에 다른 모델을 직접 말하면 그 말이 이 표보다 우선합니다.

```text
orca orchestration worker-start --agent claude --model claude-opus-5-5 --effort xhigh --worktree current --task-title "<짧은 제목>" --spec-file <spec 파일>
orca orchestration worker-start --agent claude --model claude-fable-5-2 --effort xhigh --worktree current --task-title "<짧은 제목>" --spec-file <spec 파일>
orca orchestration worker-start --agent codex --model gpt-6.1-terra --effort xhigh --worktree current --task-title "<짧은 제목>" --spec-file <spec 파일>
```

spec은 길고 따옴표·백틱이 많아 PowerShell에서 `--spec`으로 넘기면 깨지기 쉬우니, 세션 스크래치패드 폴더에
파일로 쓰고 `--spec-file <절대 경로>`로 넘깁니다.

## 3. 어느 종류에 속하나

- **일반 코드**: `sulsul-core`, `sulsul-stt-*` 엔진 크레이트, `sulsul-cli`, 테스트, `Cargo.toml`·의존성, 그리고
  `sulsul-app` 안이라도 화면을 그리지 않는 모듈(`config_sync.rs`, `session.rs`, `engines.rs`, `accel.rs`,
  `error_text.rs`, `pages/*/job.rs` 같은 작업 스레드).
- **GUI**: `sulsul-app`의 `pages/`, `widgets/`, `theme.rs`, `fonts.rs`, `icons.rs`, `app.rs`의 화면 구성과 레이아웃,
  그 화면에 쓰는 `locales/ko.yml` 문구.
- **실행 테스트**: 앱을 실제로 띄우고 computer use로 눌러 보며 동작과 화면을 확인하는 일.
  일반 코드·GUI 워커는 빌드·clippy·테스트까지만 하고, 앱을 띄워 눌러 보는 확인은 실행 테스트 워커에게 넘깁니다.

애매하면 `egui::Ui`를 받아 무언가를 그리는 코드는 GUI, 그렇지 않으면 일반 코드로 봅니다.

## 4. 나누는 법

- 병렬 워커끼리 같은 파일을 고치지 않도록 spec마다 **고쳐도 되는 파일·크레이트**를 적습니다.
  `Cargo.toml`, `Cargo.lock`, `locales/ko.yml`, `sulsul-core`의 공개 타입처럼 여럿이 건드리기 쉬운 곳은 한 워커에게만 맡깁니다.
- 로직과 GUI가 함께 바뀌는 일은 코어 API(타입·함수 시그니처)를 먼저 정해 양쪽 spec에 똑같이 적고 동시에 시작합니다.
  미리 정하기 어려우면 일반 코드 워커를 먼저 끝내고 GUI 워커를 시작합니다(순서가 필요하면 orchestration의 Task
  DAG `--deps`, `references/coordinator-loop.md`).
- 기본은 `--worktree current`입니다. 같은 작업 트리에서 여러 워커가 동시에 cargo를 돌리면 빌드 폴더 잠금 때문에
  서로 기다리는데, 이건 정상입니다. 다만 다른 워커가 고치는 중인 크레이트에 의존하면 빌드가 잠깐 깨질 수 있으니,
  spec에 "빌드 오류가 내 범위 밖 파일에서 나면 고치지 말고 잠시 뒤 다시 시도, 계속되면 `ask`로 묻기"를 넣습니다.
  변경이 크게 얽혀 이걸로 안 될 때만 `--worktree new-child --name <짧은 이름>`을 씁니다.
- 실행 테스트 워커는 코드 워커들이 모두 끝나고, 감독자가 합쳐진 상태로 빌드가 되는 것을 확인한 뒤에 시작합니다.

## 5. 모든 워커 spec에 넣을 것

orchestration의 "Task spec" 원칙(대상, 바꿀 것, 제약, 고쳐도 되는 범위, 끝났음을 보여 줄 증거)에 더해:

- "먼저 저장소 루트의 `AGENTS.md`를 읽어라." (Codex는 저절로 읽지만 Claude 워커는 읽지 않으므로 꼭 적습니다.)
- 완료 기준: 고친 크레이트의 `cargo fmt --all`, `cargo clippy -p <크레이트> --all-targets`, `cargo test -p <크레이트>` 통과.
  모두 기본 빌드로만 하고 CPU 전용 빌드(`--no-default-features`)는 spec에 넣지 않습니다. `VULKAN_SDK`가 셸에
  없으면 설치 경로(보통 `C:\VulkanSDK\<버전>`)를 지정하게 하고, 그래도 안 되면 CPU 빌드로 우회하지 말고 보고.
- 커밋하지 말 것. 보고에는 바꾼 파일 목록, 돌린 명령과 결과 요약, 남은 문제를 넣을 것.

GUI 워커 spec에는 추가로: 색·글자 크기·모서리는 `theme.rs`의 토큰만 쓰기, `widgets/`에 있는 위젯 먼저 쓰기,
다크 모드 기본, 창 최소 크기(780×540)에서도 안 깨지게, 화면 문구는 `t!("키")`와 `locales/ko.yml`로.

## 6. 실행 테스트 워커 spec

- computer use는 `orca-computer-use` 스킬로 합니다(`orca skills get computer-use`로 가이드 읽기).
- 실행은 사용자의 실제 설정·모델 폴더를 건드리지 않도록 반드시 `SULSUL_HOME`을 저장소의 `.sulsul-home/`으로 지정합니다.
  PowerShell: `$env:SULSUL_HOME = "<저장소 절대 경로>\.sulsul-home"; cargo run -p sulsul-app`
- 첫 빌드는 whisper.cpp 때문에 오래 걸리니, 감독자가 미리 `cargo build -p sulsul-app`을 해 두고 spec에 그렇다고 적습니다.
- 개발 빌드에는 가짜 엔진 `mock`이 있으니 모델 없이 흐름을 볼 때는 그걸 씁니다. 수 GB짜리 모델 내려받기와
  클라우드 API 호출은 spec에 시킨 경우에만 합니다.
- 확인할 시나리오를 spec에 번호를 붙여 구체적으로 적습니다(어느 화면에서 무엇을 눌렀을 때 무엇이 보여야 하는지).
  기본 창 크기(960×660)와 최소 크기(780×540) 둘 다에서 봅니다.
- 코드는 고치지 않습니다. 시나리오마다 통과·실패, 재현 단계, 기대 결과와 실제 결과, 스크린샷 경로를 보고하고,
  끝나면 앱을 닫습니다.

## 7. 결과를 받은 뒤

- 각 `worker_done`의 증거를 요약하고, 맡긴 범위 밖 파일을 고치지 않았는지 `git status`/`git diff`로 확인합니다.
- 여러 워커의 변경이 합쳐진 상태는 각자의 테스트로 확인되지 않았으므로 `cargo test --workspace`를 한 번 돌립니다.
- 고칠 것은 직접 고치지 말고 orchestration대로 해당 워커에게 `send`로 돌려보냅니다. 실행 테스트에서 나온 문제는
  원인에 따라 GUI 워커나 일반 코드 워커에게 보냅니다.
- 사용자에게는 Task별로 결과, 증거, 남은 문제를 보고합니다. 커밋은 사용자가 요청할 때 `AGENTS.md`의 커밋 규칙대로
  기능 단위로 나눠 합니다.
