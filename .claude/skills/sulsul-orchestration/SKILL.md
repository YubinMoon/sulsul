---
name: sulsul-orchestration
description: >-
  술술(sulsul) 저장소에서 Orca `orchestration` 스킬로 워커를 띄울 때 그 위에 얹는 확장 스킬.
  orchestration의 절차는 그대로 쓰고, 작업 종류별로 어떤 워커(agent·model·effort)에게 맡길지,
  워커마다 worktree를 나눠 완료 목표만 넘기는 법, 병합 워커로 합치는 법, computer use 실행 테스트 방법을 정한다.
  이 저장소에서 /orchestration, Orca 워커, 병렬 워커, "워커에게 맡겨", "Codex한테 테스트시켜",
  GUI 작업 위임, worktree 병합, 앱 실행 테스트를 언급하거나 orchestration 스킬을 쓰려는 순간에는
  orchestration만 쓰지 말고 반드시 이 스킬도 함께 불러라.
---

# 술술 오케스트레이션

이 스킬은 `orchestration` 스킬을 이어받아 술술 저장소용 규칙을 덧붙입니다.
`worker-start`, 턴 끝내기, 깨어났을 때 처리, `send`/`reply`, 복구 같은 **절차는 전부 orchestration을 따르고**,
여기서는 **누구에게 무엇을 어떤 모델로 맡길지, 어떻게 나누고 합칠지**만 정합니다. 둘이 어긋나면 이 스킬이 우선합니다.

## 1. 먼저 orchestration 불러오기

1. `orchestration` 스킬을 불러 그 안내대로 `orca skills get orchestration`으로 Orca 버전에 맞는 가이드를 읽습니다.
   이 세션에서 이미 읽었다면 다시 읽지 않습니다.
2. 할 일을 아래 종류로 나누고, 워커마다 표의 옵션을 붙여 `worker-start` 합니다.

## 2. 워커 배정

| 종류                    | `--agent` | `--model`           | `--effort` | `--worktree`                | 띄우는 수                      |
| --------------------- | --------- | ------------------- | ---------- | --------------------------- | -------------------------- |
| 일반 코드                 | `claude`  | `claude-opus-5-5`   | `xhigh`    | `new-child --name <짧은 이름>` | 목표마다 하나씩, 병렬               |
| GUI                   | `claude`  | `claude-opus-5-5`   | `xhigh`    | `new-child --name <짧은 이름>` | 보통 하나 (화면이 완전히 나뉘면 여럿)     |
| 병합                    | `claude`  | `claude-sonnet-5-5` | `xhigh`    | `current`                   | 하나 (코드 워커가 모두 끝난 뒤)        |
| 실행 테스트 (computer use) | `codex`   | `gpt-6.1-terra`     | `high`     | `current`                   | 하나 (병합과 감독자 확인이 끝난 뒤)      |

orchestration 가이드에는 "사용자가 모델을 말했을 때만 `--model`을 붙인다"고 되어 있지만, 이 표가 사용자가 정한
모델이므로 **항상 `--model`과 `--effort`를 붙입니다**. 빼면 각 에이전트의 기본 설정 모델로 떠서 배정이 무너집니다.
사용자가 어떤 작업에 다른 모델을 직접 말하면 그 말이 이 표보다 우선합니다.

```text
orca orchestration worker-start --agent claude --model claude-opus-5-5 --effort xhigh --worktree new-child --name <짧은 이름> --task-title "<짧은 제목>" --spec-file <spec 파일>
orca orchestration worker-start --agent claude --model claude-sonnet-5-5 --effort xhigh --worktree current --task-title "worktree 병합" --spec-file <spec 파일>
orca orchestration worker-start --agent codex --model gpt-6.1-terra --effort high --worktree current --task-title "<짧은 제목>" --spec-file <spec 파일>
```

spec은 따옴표·백틱이 많아 PowerShell에서 `--spec`으로 넘기면 깨지기 쉬우니, 세션 스크래치패드 폴더에
파일로 쓰고 `--spec-file <절대 경로>`로 넘깁니다.

## 3. 어느 종류에 속하나

- **일반 코드**: `sulsul-core`, `sulsul-stt-*` 엔진 크레이트, `sulsul-cli`, 테스트, `Cargo.toml`·의존성, 그리고
  `sulsul-app` 안이라도 화면을 그리지 않는 모듈(`config_sync.rs`, `session.rs`, `engines.rs`, `accel.rs`,
  `error_text.rs`, `pages/*/job.rs` 같은 작업 스레드).
- **GUI**: `sulsul-app`의 `pages/`, `widgets/`, `theme.rs`, `fonts.rs`, `icons.rs`, `app.rs`의 화면 구성과 레이아웃,
  그 화면에 쓰는 `locales/ko.yml` 문구.
- **병합**: 코드 워커들의 worktree 변경을 감독자 작업 트리에 합치고 충돌을 해결하는 일.
- **실행 테스트**: 앱을 실제로 띄우고 computer use로 눌러 보며 동작과 화면을 확인하는 일.
  일반 코드·GUI 워커는 빌드·clippy·테스트까지만 하고, 앱을 띄워 눌러 보는 확인은 실행 테스트 워커에게 넘깁니다.

애매하면 `egui::Ui`를 받아 무언가를 그리는 코드는 GUI, 그렇지 않으면 일반 코드로 봅니다.

## 4. 나누는 법: 완료 목표 + worktree

- 코드 워커(일반 코드·GUI)는 **각자 자기 worktree**(`--worktree new-child --name <짧은 이름>`)에서 일합니다.
  파일이 겹쳐도 괜찮습니다. 충돌은 병합 워커가 해결합니다.
- 감독자는 할 일을 **완료 목표**(끝났을 때 사용자에게 무엇이 어떻게 동작해야 하는지) 단위로 나눌 만큼만 코드를 봅니다.
  구현 방법, 고쳐도 되는 파일 목록, 미리 맞춘 API·뼈대는 만들지 않습니다. 코드 분석과 설계는 각 워커가 합니다.
- 다만 사용자가 정한 동작(예: 기본 단축키, 저장 위치, 닫기 버튼 동작)과 `AGENTS.md`의 확정된 결정은 목표에 적습니다.
  두 워커가 같은 것을 서로 다르게 정할 수 있는 지점(예: 설정 키 이름, 화면이 쓸 상태 값)이 보이면, 한쪽이 정하고
  다른 쪽이 따르게 하기보다 목표에 "병합 워커가 맞출 것"이라고 알려 두는 정도로 둡니다.
- worktree는 **마지막 커밋**에서 갈라집니다. 감독자 작업 트리의 커밋하지 않은 변경은 보이지 않습니다.
  앞 작업 결과에 기대는 작업은 병합이 끝난 뒤 `--worktree current`로 띄워 합쳐진 트리에서 하게 하거나,
  사용자가 커밋을 허락했으면 커밋한 뒤 새 worktree로 띄웁니다. 순서가 필요하면 orchestration의 Task DAG `--deps`
  (`references/coordinator-loop.md`)를 씁니다.
- 새 worktree의 첫 빌드는 whisper.cpp 때문에 오래 걸립니다. 정상이니 spec에 알려 두고 기다립니다.
- 병합 워커는 코드 워커가 모두 `worker_done`을 보낸 뒤에, 실행 테스트 워커는 병합과 감독자 확인(7번)이 끝난 뒤에 시작합니다.

## 5. 코드 워커 spec에 넣을 것

길게 쓰지 않습니다. 다음만 넣습니다.

- "먼저 저장소 루트의 `AGENTS.md`를 읽어라." (Codex는 저절로 읽지만 Claude 워커는 읽지 않으므로 꼭 적습니다.)
- 자기 worktree에서 일하고, 감독자 작업 트리(저장소 경로)는 읽기만 한다는 것. 끝나면 병합 워커가 합친다는 것.
- 완료 목표: 번호를 붙여, 끝났을 때 동작해야 하는 것. 사용자가 정한 동작과 제약.
- 완료 기준: 고친 크레이트의 `cargo fmt --all`, `cargo clippy -p <크레이트> --all-targets`, `cargo test -p <크레이트>` 통과.
  모두 기본 빌드로만 하고 CPU 전용 빌드(`--no-default-features`)는 spec에 넣지 않습니다. `VULKAN_SDK`가 셸에
  없으면 설치 경로(보통 `C:\VulkanSDK\<버전>`)를 지정하게 하고, 그래도 안 되면 CPU 빌드로 우회하지 말고 보고.
- 커밋하지 말 것. 보고에는 worktree 경로, 바꾼 파일 목록, 병합 워커가 알아야 할 것(겹칠 만한 파일과 그 의도),
  실행 테스트 워커가 볼 시나리오, 돌린 명령과 결과 요약, 남은 문제를 넣을 것.

GUI 워커 spec에는 추가로: 색·글자 크기·모서리는 `theme.rs`의 토큰만 쓰기, `widgets/`에 있는 위젯 먼저 쓰기,
다크 모드 기본, 창 최소 크기(780×540)에서도 안 깨지게, 화면 문구는 `t!("키")`와 `locales/ko.yml`로.

## 6. 병합 워커 spec

- "먼저 저장소 루트의 `AGENTS.md`를 읽어라."
- 합칠 worktree 경로 목록과 순서, 각 워커의 `worker_done` 보고(바꾼 파일, 의도, 겹칠 만한 곳)를 그대로 붙여 줍니다.
- 기준점은 worktree가 갈라진 커밋(`git merge-base`)입니다. 워커 변경도 감독자 작업 트리 변경도 커밋되어 있지 않으므로,
  파일마다 기준점·감독자 트리·worktree 세 판을 놓고 3방향 병합(`git merge-file` 등)합니다. 새 파일은 가져오고,
  지운 파일은 지웁니다. `Cargo.lock`은 손으로 합치지 말고 `Cargo.toml`을 합친 뒤 `cargo`가 다시 만들게 합니다.
- 충돌은 양쪽 의도를 모두 살려 해결합니다. 두 워커가 같은 것(타입 이름, 설정 키, 문구 키)을 서로 다르게 만들었으면
  하나로 맞추고 쓰는 곳을 고칩니다. 한쪽 기능을 빼야만 풀리는 충돌은 고치지 말고 `ask`로 묻습니다.
- 감독자 작업 트리에서만 고치고 worktree 쪽은 건드리지 않습니다. 커밋하지 않습니다.
- 완료 기준: `cargo fmt --all`, `cargo clippy --workspace --all-targets` 경고 없음, `cargo test --workspace` 통과(기본 빌드).
- 보고에는 합친 파일 목록, 충돌이 난 곳과 어떻게 풀었는지, 맞춘 이름, 돌린 명령과 결과, 남은 문제를 넣습니다.

## 7. 실행 테스트 워커 spec

- computer use는 `orca-computer-use` 스킬로 합니다(`orca skills get computer-use`로 가이드 읽기).
- 실행은 사용자의 실제 설정·모델 폴더를 건드리지 않도록 반드시 `SULSUL_HOME`을 저장소의 `.sulsul-home/`으로 지정합니다.
  PowerShell: `$env:SULSUL_HOME = "<저장소 절대 경로>\.sulsul-home"; cargo run -p sulsul-app`
- 첫 빌드는 whisper.cpp 때문에 오래 걸리니, 감독자가 미리 `cargo build -p sulsul-app`을 해 두고 spec에 그렇다고 적습니다.
- 개발 빌드에는 가짜 엔진 `mock`이 있으니 모델 없이 흐름을 볼 때는 그걸 씁니다. 수 GB짜리 모델 내려받기와
  클라우드 API 호출은 spec에 시킨 경우에만 합니다.
- 확인할 시나리오를 spec에 번호를 붙여 구체적으로 적습니다(어느 화면에서 무엇을 눌렀을 때 무엇이 보여야 하는지).
  코드 워커들이 보고한 시나리오를 모아 씁니다. 기본 창 크기(960×660)와 최소 크기(780×540) 둘 다에서 봅니다.
- 코드는 고치지 않습니다. 시나리오마다 통과·실패, 재현 단계, 기대 결과와 실제 결과, 스크린샷 경로를 보고하고,
  끝나면 앱을 닫습니다.

## 8. 결과를 받은 뒤

- 코드 워커의 `worker_done`마다 증거를 요약합니다. 병합 전에 나온 문제는 그 워커에게 `send`로 돌려보내 자기 worktree에서 고치게 합니다.
- 병합 워커가 끝나면 합쳐진 트리에서 `cargo test --workspace`를 한 번 돌려 확인합니다.
- 병합 뒤에 나온 문제(실행 테스트 결과 등)는 worktree가 이미 낡았으므로, 원인 종류(일반 코드·GUI)에 맞는 워커를
  `--worktree current`로 새로 띄워 합쳐진 트리에서 고치게 합니다. 감독자가 직접 고치지 않습니다.
- 병합이 확인된 worktree 워커는 `worker-list`의 `next`를 따라 정리합니다(새 worktree 워커는 저절로 닫히지 않습니다).
  worktree 폴더를 지우는 것은 병합이 확실할 때만 하고, 애매하면 남겨 두고 사용자에게 알립니다.
- 사용자에게는 Task별로 결과, 증거, 남은 문제를 보고합니다. 커밋은 사용자가 요청할 때 `AGENTS.md`의 커밋 규칙대로
  기능 단위로 나눠 합니다.
