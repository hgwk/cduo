[English](README.md) | [한국어](README.ko.md)

# cduo

`cduo`는 Claude Code 또는 Codex 세션 두 개를 split `tmux` 워크스페이스 안에서 실행하되, relay 제어는 `tmux`가 아니라 `cduo`가 맡는 도구입니다.

## 하는 일

- 두 개의 pane을 가진 `tmux` 세션 생성
- `claude`와 공식 OpenAI `codex` CLI 지원
- 컨트롤러 프로세스가 두 agent를 direct PTY로 직접 실행
- `tmux`를 제어 채널로 쓰지 않고 split pane 안에 두 세션을 그대로 보여 줌
- Claude는 `Stop` hook 기반 completion relay
- Codex는 rollout JSONL 기반 completion relay
- `.claude/settings.local.json`과 `CLAUDE.md`를 통한 Claude 프로젝트 컨텍스트 관리
- 파괴적 변경 전에 자동 백업 생성

## 요구 사항

- `tmux`
- Claude 세션용 `claude` CLI
- Codex 세션용 공식 OpenAI `codex` CLI

Codex CLI가 아직 없다면:

```bash
npm install -g @openai/codex@latest
```

## 지원 정책

- 공식 지원 플랫폼: macOS, Linux
- 현재 workspace 모델: split `tmux`
- Windows는 현재 패키지 제품 경로에서 지원하지 않습니다
- 설치 문제가 있으면 먼저 `cduo doctor`를 실행하세요

## 빠른 시작

글로벌 설치:

```bash
npm install -g @hgwk/cduo
```

처음 한 번은 환경 점검:

```bash
cduo doctor
```

공식 패키지 기준:

- npm 패키지: `@hgwk/cduo`
- 설치 후 실행 명령: `cduo`

설치 후 실행 명령은 그대로입니다:

```bash
cduo
```

Claude:

```bash
cd /path/to/project
cduo init
cduo claude
```

Codex:

```bash
cd /path/to/project
cduo codex
```

동작 요약:

- `cduo`는 `cduo start`와 같습니다
- `cduo start`의 기본 agent는 Claude입니다
- `cduo init`은 Claude 프로젝트 컨텍스트가 필요할 때만 쓰면 됩니다
- Codex는 `cduo init` 없이도 동작합니다
- 예전 `tmux` 토큰은 계속 받지만, 이제는 tmux가 layout만 담당합니다

## 일상 사용 흐름

```bash
cduo doctor
cduo claude
```

이후 같은 프로젝트에서:

```bash
cduo resume
cduo status
cduo stop
```

`cduo`를 비대화형 프로세스에서 시작하면 바로 attach하지 않고 workspace만 만든 뒤 `cduo resume ...` 명령을 안내합니다.

운영 메모:

- `cduo resume`는 실행 중인 `tmux` workspace에 붙기 때문에 대화형 터미널이 필요합니다
- `cduo status --verbose`는 진단용으로 session id, agent, hook port, 생성 시각, pane별 attach port 정보를 더 보여 줍니다

Workspace 선택 규칙:

- `cduo resume`와 `cduo stop`는 먼저 현재 프로젝트에 딱 하나 있는 workspace를 우선 선택합니다
- 현재 프로젝트에 맞는 workspace가 여러 개면 자동으로 고르지 않고 명시적으로 하나를 선택하라고 멈춥니다
- 현재 프로젝트 workspace가 없고 전체 활성 workspace가 하나뿐이면 그 workspace를 자동으로 사용합니다
- 명시적 selector는 `cduo status`에 보이는 session name, session id, project name, 또는 고유 prefix로 지정할 수 있습니다

## 명령어

| 명령어 | 설명 |
| --- | --- |
| `cduo` | Claude 기본값으로 tmux split 워크스페이스 시작 |
| `cduo help` 또는 `cduo --help` | 명령 도움말 표시 |
| `cduo start [claude\|codex] [yolo\|--yolo\|--full-access] [--new]` | 선택한 agent workspace를 시작하거나 다시 연결 |
| `cduo claude [yolo\|--yolo\|--full-access] [--new]` | Claude workspace를 시작하거나 다시 연결 |
| `cduo codex [yolo\|--yolo\|--full-access] [--new]` | Codex workspace를 시작하거나 다시 연결 |
| `cduo doctor` | 머신 설정과 현재 프로젝트 준비 상태 점검 |
| `cduo resume [session]` | 현재 프로젝트 workspace 또는 지정한 workspace에 다시 연결 |
| `cduo status [--verbose]` | 활성 cduo workspace 표시 |
| `cduo stop [session]` | 현재 프로젝트 workspace 또는 지정한 workspace 중지 |
| `cduo init` | Claude `Stop` hook을 보장하고 `CLAUDE.md`에 orchestration 내용을 생성하거나 앞에 추가 |
| `cduo init --force` | `.claude/settings.local.json`과 `CLAUDE.md`를 덮어씀 |
| `cduo backup` | 현재 프로젝트의 orchestration 관련 파일 백업 |
| `cduo update` | 글로벌 CLI를 `npm install -g @hgwk/cduo@latest`로 업데이트 |
| `cduo version` 또는 `cduo --version` | 설치된 cduo 버전 표시 |
| `cduo uninstall` | 주입된 Claude hook과 orchestration 컨텍스트 제거 |

## 인자 규칙

- 한 세션에는 agent를 하나만 선택할 수 있습니다
- `yolo`와 `--yolo`는 같은 의미입니다
- `yolo` 또는 `--yolo`는 `--full-access`와 함께 쓸 수 없습니다
- 기본적으로 `cduo claude`와 `cduo codex`는 같은 프로젝트, 같은 agent, 같은 접근 모드의 기존 workspace에 다시 연결합니다
- 같은 프로젝트와 같은 agent로 새 workspace를 일부러 더 만들고 싶을 때만 `--new`를 사용합니다
- `start` 뒤에서는 `claude`나 `codex`를 뒤쪽 어느 위치에 둬도 됩니다
- 예상하지 않은 추가 start 인자는 무시하지 않고 오류로 거부합니다
- `tmux`는 하위 호환용으로만 남아 있고 동작은 바꾸지 않습니다

유효한 예시:

```bash
cduo
cduo update
cduo start
cduo start codex
cduo start tmux codex
cduo claude yolo
cduo codex --yolo
cduo codex --full-access
cduo codex --new
```

거부되는 예시:

```bash
cduo start claude codex
cduo codex nonsense
```

## 접근 모드

- `cduo claude --full-access`는 Claude를 `--permission-mode bypassPermissions`로 실행합니다
- `cduo claude yolo`는 Claude를 `--dangerously-skip-permissions`로 실행합니다
- `cduo codex --full-access`는 설치된 공식 OpenAI CLI가 제공하는 full-access 대응 모드로 실행합니다
- `cduo codex yolo`는 설치된 공식 OpenAI CLI가 제공하는 auto-approval 대응 모드로 실행합니다

Codex 옵션 매핑은 설치된 공식 CLI 버전에 따라 달라집니다.

- 최신 계열은 `--yolo`, `--sandbox danger-full-access`를 사용합니다
- 구형 공식 계열은 `--approval-mode full-auto`, `--dangerously-auto-approve-everything`를 사용합니다

`cduo`는 실행 전에 두 공식 변형을 모두 감지합니다.

지원하는 OpenAI Codex CLI 옵션 참고 문서:

- [Codex CLI reference](https://developers.openai.com/codex/cli/reference)
- [Agent approvals & security](https://developers.openai.com/codex/agent-approvals-security)

## 에이전트별 동작

| 에이전트 | 실행 명령 | completion 감지 방식 | `start`가 수정하는 파일 |
| --- | --- | --- | --- |
| Claude | `claude` | `Stop` hook + Claude transcript JSONL | Claude `Stop` hook을 생성하거나 병합할 수 있음 |
| Codex | `codex` | Codex rollout JSONL | 없음 |

Codex를 선택하면 `cduo`는 현재 `PATH`의 `codex`가 공식 OpenAI CLI인지 먼저 확인합니다.

## 명령어가 수정하는 파일

`cduo init`은 다음 파일을 생성하거나 갱신할 수 있습니다.

```text
your-project/
├── .cduo/
│   └── backups/
├── .claude/
│   └── settings.local.json
├── CLAUDE.md
└── ...
```

명령별 동작:

- `cduo init`은 `.claude/settings.local.json`과 `CLAUDE.md`를 함께 관리합니다
- `cduo start`와 `cduo claude ...`는 Claude를 선택한 경우에만 Claude `Stop` hook을 관리합니다
- `cduo codex ...`는 프로젝트 파일을 수정하지 않습니다
- `cduo backup`은 `.cduo/backups/` 아래에 타임스탬프 백업을 저장합니다

## Relay 구조

1. `cduo`가 내장 daemon을 시작해 workspace를 관리합니다.
2. daemon이 선택된 agent를 `TERMINAL_ID`와 `ORCHESTRATION_PORT`를 가진 direct PTY 두 개로 실행합니다.
3. `tmux`는 split UI만 제공합니다.
4. Claude는 `Stop` hook으로 completion 이벤트와 transcript 경로를 보냅니다.
5. Codex completion은 현재 workspace의 Codex rollout JSONL에서 읽습니다.
6. `MessageBus`가 source/target/content 중복 전송을 막고 `PairRouter`가 상대 pane으로 전달합니다.
7. relay 출력은 target PTY stdin에 직접 쓰고 Enter를 보냅니다. 터미널 UI 출력은 메시지 본문으로 쓰지 않습니다.

선호하는 relay 기본 포트:

- `53333`

기본 로컬 포트 대역이 이미 사용 중이면 `cduo`가 OS가 할당한 로컬 포트로 자동 fallback합니다.

필요하면 선호 기본 포트를 바꿀 수 있습니다:

```bash
PORT=8080 cduo codex
```

## 백업과 제거

수동 백업:

```bash
cduo backup
```

현재 프로젝트에서 orchestration 설정 제거:

```bash
cduo uninstall
```

`cduo uninstall`은 현재 파일을 먼저 백업한 뒤 다음을 수행합니다.

- `.claude/settings.local.json`에서 Claude `Stop` hook 제거
- cduo 템플릿과 같은 Claude 기본 권한 설정이 있으면 함께 제거
- `CLAUDE.md` 앞에 붙은 orchestration 블록 제거
- 파일이 번들 템플릿만 담고 있으면 `CLAUDE.md` 자체 삭제

설치된 CLI 업데이트:

```bash
cduo update
```

`cduo update`는 아래 명령을 감싼 편의 명령입니다.

```bash
npm install -g @hgwk/cduo@latest
```

## 트러블슈팅

메시지 relay가 안 되는 경우:

- 먼저 `cduo doctor`로 런타임 상태를 확인
- `cduo status`로 workspace와 controller가 살아 있는지 먼저 확인
- 더 깊은 진단이 필요하면 `cduo status --verbose`를 실행
- `cduo start`, `cduo resume`, `cduo status`는 진행 전에 stale workspace 메타데이터를 자동 정리합니다
- Claude는 relay 서버 로그에 hook 이벤트가 찍히는지 확인
- Codex는 현재 프로젝트에 대응하는 최근 rollout JSONL이 `~/.codex/sessions/` 아래 생기는지 확인
- target pane이 stdin을 받을 수 있어야 하며, `cduo`는 relay 텍스트를 쓴 뒤 Enter를 보냅니다
- `cduo`를 업그레이드했다면 새 controller가 반영되도록 cduo 세션을 다시 시작해야 합니다
- `tmux` 세션이 아직 살아 있는지 확인
- 같은 프로젝트에서는 보통 `cduo resume`만으로 기대한 workspace에 다시 붙어야 합니다
- workspace 시작 뒤 attach가 실패해도 workspace는 계속 살아 있는 경우가 많으니, 출력된 `cduo resume ...` 명령을 대화형 터미널에서 다시 실행하세요

Codex가 설치돼 있는데 `cduo codex`가 거부되는 경우:

- `codex --help`에 최신 공식 옵션(`--yolo`, `--ask-for-approval`, `--sandbox`)이나 구형 공식 옵션(`--approval-mode`, `full-auto`, `--dangerously-auto-approve-everything`)이 보이는지 확인
- 아니라면 `npm install -g @openai/codex@latest`로 공식 CLI 설치 또는 업데이트
- `PATH`에서 `codex`가 OpenAI 바이너리를 가리키는지 확인

`tmux` layout mode가 바로 실패하는 경우:

- macOS: `brew install tmux`
- Ubuntu/Debian: `sudo apt install tmux`
- Fedora: `sudo dnf install tmux`
- Arch: `sudo pacman -S tmux`

터미널 시작 위치가 이상한 경우:

- 원하는 프로젝트 루트에서 `cduo claude` 또는 `cduo codex` 실행

Claude에 orchestration 컨텍스트가 안 들어간 경우:

- `cduo init` 실행
- `CLAUDE.md`는 Claude 흐름에서만 관리된다는 점 확인

Claude 시작 전에 `SessionStart:startup hook error`가 보이는 경우:

- `cduo doctor`를 실행해서 `Claude startup hooks` 항목을 확인
- 이 경고는 보통 cduo의 `Stop` hook이 아니라 `claude-mem` 같은 서드파티 Claude plugin의 `SessionStart` hook에서 발생합니다
- 해당 plugin을 업데이트하거나, `SessionStart` hook이 JSON만 반환하도록 수정하세요

## 개발

소스에서 빌드:

```bash
git clone https://github.com/hgwk/cduo.git
cd cduo
cargo build --release
```

테스트 실행:

```bash
cargo test
```

로컬 E2E 스모크 테스트:

```bash
scripts/e2e-test.sh
```

릴리즈 바이너리는 `target/release/cduo`에 생성됩니다.

프로젝트 구조:

```text
cduo/
├── src/
│   ├── main.rs           # CLI 진입점
│   ├── cli.rs            # 명령 정의와 파싱
│   ├── daemon.rs         # 내장 daemon과 세션 관리
│   ├── hook.rs           # Claude HTTP hook 서버
│   ├── pty.rs            # PTY 관리(portable-pty)
│   ├── message.rs        # relay 메시지 모델
│   ├── message_bus.rs    # 중복 제거 메시지 버스
│   ├── pair_router.rs    # 1:1 라우팅 정책
│   ├── session.rs        # 세션 메타데이터와 저장
│   ├── tmux.rs           # tmux 세션 헬퍼
│   └── transcripts/      # 에이전트 transcript 리더
├── templates/
│   ├── claude-settings.json
│   └── orchestration.md
├── npm/
│   ├── install.js
│   └── package.json
├── scripts/
│   └── e2e-test.sh
├── docs/
│   └── architecture.md
├── Cargo.toml
├── Cargo.lock
├── .github/
│   └── workflows/
│       ├── rust-ci.yml
│       └── release.yml
├── LICENSE
├── README.md
└── README.ko.md
```

배포 흐름:

- GitHub 저장소: `hgwk/cduo`
- npm 패키지: `@hgwk/cduo`
- GitHub Releases가 플랫폼별 Rust 바이너리를 호스팅합니다
- `.github/workflows/rust-ci.yml`가 push와 pull request에서 테스트를 실행합니다
- `.github/workflows/release.yml`가 버전 태그에서 바이너리를 빌드하고 npm 패키지를 배포합니다
- npm 패키지는 설치 시 현재 플랫폼에 맞는 바이너리를 다운로드하는 얇은 wrapper입니다
- release 태그 버전은 `Cargo.toml`과 `npm/package.json` 버전과 같아야 합니다

## 라이선스

MIT
