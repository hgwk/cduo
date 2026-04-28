[English](README.md) | [한국어](README.ko.md)

# cduo

`cduo`는 Claude Code 또는 Codex 세션 두 개를 native split terminal UI 안에서 실행하고, agent 간 relay도 직접 제어하는 도구입니다.

## 하는 일

- 두 개의 pane을 가진 native terminal UI 생성
- `claude`와 공식 OpenAI `codex` CLI 지원
- 컨트롤러 프로세스가 두 agent를 direct PTY로 직접 실행
- `ratatui` + `vt100`으로 두 세션을 직접 렌더링
- Claude는 `Stop` hook 기반 completion relay
- Codex는 rollout JSONL 기반 completion relay
- `.claude/settings.local.json`과 `CLAUDE.md`를 통한 Claude 프로젝트 컨텍스트 관리
- 파괴적 변경 전에 자동 백업 생성

## 요구 사항

- Claude 세션용 `claude` CLI
- Codex 세션용 공식 OpenAI `codex` CLI

Codex CLI가 아직 없다면:

```bash
npm install -g @openai/codex@latest
```

## 지원 정책

- 공식 지원 플랫폼: macOS, Linux
- 현재 session 모델: native split terminal UI
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

Claude + Codex 혼합:

```bash
cd /path/to/project
cduo init   # Claude relay에 필요
cduo start claude codex
```

동작 요약:

- `cduo`는 `cduo start`와 같습니다
- `cduo start`의 기본 agent는 Claude입니다
- `cduo init`은 Claude 프로젝트 컨텍스트가 필요할 때만 쓰면 됩니다
- Codex는 `cduo init` 없이도 동작합니다
- native session은 foreground 프로세스입니다. UI를 종료하면 agent들도 종료됩니다
- native control에는 pane focus 전환, relay 수동 전송/일시중지/방향 토글, relay 상태 표시, layout 전환이 포함됩니다

## 일상 사용 흐름

```bash
cduo doctor
cduo start claude codex
```

native UI는 foreground로 실행됩니다. 종료는 UI 안에서 `Ctrl-Q`를 사용합니다.

native UI 조작:

- `Ctrl-W`: pane focus 전환
- `Ctrl-Shift-W`: 반대 방향으로 pane focus 전환
- `Ctrl-R`: 현재 pane 내용을 peer pane으로 수동 relay
- `Ctrl-X`: relay 일시중지 중 쌓인 queued write 비우기
- `Ctrl-1`: A -> B relay 켜기/끄기
- `Ctrl-2`: B -> A relay 켜기/끄기
- `Ctrl-G`: 최근 relay log/status 표시
- `Ctrl-Z`: layout preset/maximize mode 순환
- `Ctrl-P`: 자동 relay 전달 일시중지/재개
- `Ctrl-L`: 좌우/상하 split 전환
- `Ctrl-Q`: native UI 종료 및 두 agent 중지
- `PageUp` / `PageDown`: 현재 focus pane 스크롤
- 마우스 휠: 커서 아래 pane 스크롤
- 마우스 드래그: 한 pane 안에서만 텍스트 선택, 버튼을 놓으면 OSC52로 클립보드 복사

`CDUO_RELAY_PREFIX`를 설정하면 relay 메시지 앞에 짧은 지시문을 자동으로 붙일 수 있습니다.

## 명령어

| 명령어 | 설명 |
| --- | --- |
| `cduo` | Claude 기본값으로 native split UI 시작 |
| `cduo help` 또는 `cduo --help` | 명령 도움말 표시 |
| `cduo start [claude\|codex] [claude\|codex] [--split columns\|rows] [--yolo\|--full-access] [--new]` | native split UI를 시작하며, 두 번째 agent는 pane B를 선택 |
| `cduo claude [claude\|codex] [--split columns\|rows] [--yolo\|--full-access] [--new]` | pane A를 Claude로 시작 |
| `cduo codex [claude\|codex] [--split columns\|rows] [--yolo\|--full-access] [--new]` | pane A를 Codex로 시작 |
| `cduo doctor` | 머신 설정과 현재 프로젝트 준비 상태 점검 |
| `cduo status [--verbose]` | native foreground-session 동작 안내 |
| `cduo init` | Claude `Stop` hook을 보장하고 `CLAUDE.md`에 orchestration 내용을 생성하거나 앞에 추가 |
| `cduo init --force` | `.claude/settings.local.json`과 `CLAUDE.md`를 덮어씀 |
| `cduo backup` | 현재 프로젝트의 orchestration 관련 파일 백업 |
| `cduo update` | 글로벌 CLI를 `npm install -g @hgwk/cduo@latest`로 업데이트 |
| `cduo version` 또는 `cduo --version` | 설치된 cduo 버전 표시 |
| `cduo uninstall` | 주입된 Claude hook과 orchestration 컨텍스트 제거 |

## 인자 규칙

- `cduo start`는 pane A용 agent 하나와 pane B용 peer agent 하나를 선택적으로 받습니다
- `--yolo`는 `--full-access`와 함께 쓸 수 없습니다
- native mode는 실행할 때마다 새 foreground session을 시작합니다. background workspace, attach, resume은 없습니다
- `--new` / `--new-session`은 CLI 호환성을 위해 받지만, 현재 native mode에서는 no-op입니다
- 예상하지 않은 추가 start 인자는 무시하지 않고 오류로 거부합니다

유효한 예시:

```bash
cduo
cduo update
cduo start
cduo start codex
cduo start claude codex
cduo claude codex
cduo codex claude
cduo codex claude --split rows
cduo start --new claude codex
cduo claude --yolo
cduo codex --yolo
cduo codex --full-access
cduo codex --new
```

거부되는 예시:

```bash
cduo start claude codex claude
cduo codex nonsense
```

## 접근 모드

- `cduo claude --full-access`는 Claude를 `--permission-mode bypassPermissions`로 실행합니다
- `cduo claude --yolo`는 Claude를 `--dangerously-skip-permissions`로 실행합니다
- `cduo codex --full-access`는 설치된 공식 OpenAI CLI가 제공하는 full-access 대응 모드로 실행합니다
- `cduo codex --yolo`는 설치된 공식 OpenAI CLI가 제공하는 auto-approval 대응 모드로 실행합니다

Codex 옵션 매핑은 설치된 공식 CLI 기준입니다.

- `--full-access`는 Codex를 `--sandbox danger-full-access --ask-for-approval never`로 실행합니다
- `--yolo`는 Codex를 `--dangerously-bypass-approvals-and-sandbox`로 실행합니다

지원하는 OpenAI Codex CLI 옵션 참고 문서:

- [Codex CLI reference](https://developers.openai.com/codex/cli/reference)
- [Agent approvals & security](https://developers.openai.com/codex/agent-approvals-security)

## 에이전트별 동작

| 에이전트 | 실행 명령 | completion 감지 방식 | `start`가 수정하는 파일 |
| --- | --- | --- | --- |
| Claude | `claude` | `Stop` hook + Claude transcript JSONL | 없음. hook 설치는 `cduo init`이 담당 |
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
- `cduo start`, `cduo claude ...`, `cduo codex ...`는 프로젝트 파일을 수정하지 않습니다
- `cduo backup`은 `.cduo/backups/` 아래에 타임스탬프 백업을 저장합니다

## Relay 구조

1. `cduo`가 native split-pane TUI를 시작합니다.
2. native runtime이 선택된 agent를 `TERMINAL_ID`와 `ORCHESTRATION_PORT`를 가진 direct PTY 두 개로 실행합니다.
3. `ratatui` + `vt100`이 두 PTY를 직접 렌더링합니다. tmux fallback은 없습니다.
4. Claude는 `Stop` hook으로 completion 이벤트와 transcript 경로를 보냅니다.
5. Codex completion은 현재 workspace의 Codex rollout JSONL에서 읽습니다.
6. `MessageBus`가 source/target/content 중복 전송을 막고 `PairRouter`가 상대 pane으로 전달합니다.
7. relay 출력은 target PTY stdin에 직접 쓰고 Enter를 보냅니다. 터미널 UI 출력은 메시지 본문으로 쓰지 않습니다.

선호하는 relay 기본 포트:

- `53333`

기본 로컬 포트 대역이 이미 사용 중이면 `cduo`가 OS가 할당한 로컬 포트로 자동 fallback합니다.

필요하면 선호 기본 포트를 바꿀 수 있습니다:

```bash
CDUO_PORT=8080 cduo codex
```

호스팅 환경 호환을 위해 `PORT`도 받지만, `CDUO_PORT`가 우선합니다.

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
- native UI에서 두 pane이 모두 보이는지 확인
- Claude pane이 있다면 해당 프로젝트에서 `cduo init`을 한 번 실행해 `Stop` hook이 있는지 확인
- Claude는 relay 서버 로그에 hook 이벤트가 찍히는지 확인
- Codex는 현재 프로젝트에 대응하는 최근 rollout JSONL이 `~/.codex/sessions/` 아래 생기는지 확인
- target pane이 stdin을 받을 수 있어야 하며, `cduo`는 relay 텍스트를 쓴 뒤 Enter를 보냅니다
- `cduo`를 업그레이드했다면 새 바이너리가 반영되도록 native UI를 다시 시작해야 합니다

Codex가 설치돼 있는데 `cduo codex`가 거부되는 경우:

- `codex --help`에 최신 공식 옵션(`--yolo`, `--ask-for-approval`, `--sandbox`)이나 구형 공식 옵션(`--approval-mode`, `full-auto`, `--dangerously-auto-approve-everything`)이 보이는지 확인
- 아니라면 `npm install -g @openai/codex@latest`로 공식 CLI 설치 또는 업데이트
- `PATH`에서 `codex`가 OpenAI 바이너리를 가리키는지 확인

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

현재 자동 검증은 `cargo test`만 사용합니다 (단위 테스트 + `src/native/relay.rs`의 인프로세스 relay 통합 테스트). foreground native 런타임을 대상으로 한 전체 TUI E2E 하네스는 현재 연결되어 있지 않습니다.

릴리즈 바이너리는 `target/release/cduo`에 생성됩니다.

프로젝트 구조:

```text
cduo/
├── src/
│   ├── main.rs           # CLI 진입점
│   ├── cli.rs            # 명령 정의/파싱
│   ├── native/           # native split-pane TUI 런타임 (PTY + ratatui + relay 루프)
│   │   ├── runtime.rs    # 두 pane 메인 루프; hook 서버와 relay 태스크 기동
│   │   ├── pane.rs       # pane별 PTY + vt100 파서
│   │   ├── ui.rs         # vt100 → ratatui 렌더링
│   │   ├── input.rs      # 키 인코딩과 전역 동작 분류
│   │   └── relay.rs      # 인프로세스 relay 루프; 메시지 버스 구동
│   ├── relay_core.rs     # transcript 읽기 / codex rollout 발견 / dedup 등 순수 helper
│   ├── hook.rs           # Claude Stop 이벤트용 HTTP hook 서버
│   ├── message.rs        # relay 메시지 모델
│   ├── message_bus.rs    # 중복 제거 메시지 버스
│   ├── pair_router.rs    # 1:1 라우팅 정책
│   ├── session.rs        # 상태 디렉토리 결정 helper
│   ├── project.rs        # `init` / `doctor` / `backup` / `uninstall`
│   └── transcripts/      # 에이전트 transcript 리더 (claude, codex)
├── templates/
│   ├── claude-settings.json
│   └── orchestration.md
├── npm/
│   ├── install.js
│   └── package.json
├── docs/
│   ├── architecture.md
│   └── graph-routing-roadmap.md
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
- npm 패키지는 장기 `NPM_TOKEN` 없이 GitHub Actions의 npm Trusted Publishing(OIDC)으로 배포합니다
- npm 패키지는 설치 시 GitHub Releases에서 현재 플랫폼에 맞는 바이너리를 다운로드하는 얇은 wrapper입니다
- release 태그 버전은 `Cargo.toml`과 `npm/package.json` 버전과 같아야 합니다
- npm Trusted Publisher 설정:
  - Publisher: `GitHub Actions`
  - Organization or user: `hgwk`
  - Repository: `cduo`
  - Workflow filename: `release.yml`
  - Environment name: 비워둠

## 로드맵

- 현재 안정 모드: transcript 기반 native `1:1` relay
- 계획된 확장: 설정 가능한 `1:N` fan-out과 `N:N` graph routing
- 자세한 내용은 [`docs/graph-routing-roadmap.md`](docs/graph-routing-roadmap.md)를 참고하세요

## 라이선스

MIT
