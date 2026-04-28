use anyhow::Result;

use crate::cli::Agent;

#[derive(Debug, Clone, Copy)]
pub(crate) enum AccessMode {
    Default,
    Yolo,
    FullAccess,
}

impl AccessMode {
    pub(crate) fn from_flags(yolo: bool, full_access: bool) -> Result<Self> {
        match (yolo, full_access) {
            (true, true) => anyhow::bail!("Use either --yolo or --full-access, not both."),
            (true, false) => Ok(Self::Yolo),
            (false, true) => Ok(Self::FullAccess),
            (false, false) => Ok(Self::Default),
        }
    }
}

pub(crate) fn agent_args(agent: Agent, mode: AccessMode) -> &'static [&'static str] {
    match (agent, mode) {
        (Agent::Codex, AccessMode::Yolo) => &["--dangerously-bypass-approvals-and-sandbox"],
        (Agent::Codex, AccessMode::FullAccess) => &[
            "--sandbox",
            "danger-full-access",
            "--ask-for-approval",
            "never",
        ],
        (Agent::Codex, AccessMode::Default) => &[],
        (Agent::Claude, AccessMode::Yolo) => &["--dangerously-skip-permissions"],
        (Agent::Claude, AccessMode::FullAccess) => &["--permission-mode", "bypassPermissions"],
        (Agent::Claude, AccessMode::Default) => &[],
    }
}

pub(crate) fn agent_program(agent: Agent) -> &'static str {
    match agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_mode_rejects_conflicting_flags() {
        assert!(AccessMode::from_flags(true, true).is_err());
    }

    #[test]
    fn access_mode_default() {
        assert!(matches!(
            AccessMode::from_flags(false, false).unwrap(),
            AccessMode::Default
        ));
    }

    #[test]
    fn agent_args_yolo_codex() {
        let args = agent_args(Agent::Codex, AccessMode::Yolo);
        assert_eq!(args, &["--dangerously-bypass-approvals-and-sandbox"]);
    }

    #[test]
    fn agent_args_full_access_codex() {
        let args = agent_args(Agent::Codex, AccessMode::FullAccess);
        assert_eq!(
            args,
            &[
                "--sandbox",
                "danger-full-access",
                "--ask-for-approval",
                "never",
            ]
        );
    }

    #[test]
    fn agent_args_yolo_claude() {
        let args = agent_args(Agent::Claude, AccessMode::Yolo);
        assert_eq!(args, &["--dangerously-skip-permissions"]);
    }

    #[test]
    fn agent_args_full_access_claude() {
        let args = agent_args(Agent::Claude, AccessMode::FullAccess);
        assert_eq!(args, &["--permission-mode", "bypassPermissions"]);
    }

    #[test]
    fn agent_args_default_is_empty() {
        assert!(agent_args(Agent::Claude, AccessMode::Default).is_empty());
        assert!(agent_args(Agent::Codex, AccessMode::Default).is_empty());
    }
}
