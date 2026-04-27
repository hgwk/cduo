use directories::ProjectDirs;
use std::path::{Path, PathBuf};

const PROJECT_QUALIFIER: &str = "works";
const PROJECT_ORGANIZATION: &str = "higgs";
const PROJECT_APPLICATION: &str = "cduo";

/// Resolve the on-disk state directory for cduo (per-platform via the
/// `directories` crate, overridable via `CDUO_STATE_DIR`). Falls back to
/// `/tmp/cduo-state` if no project dirs are available.
pub fn get_state_root() -> PathBuf {
    if let Ok(dir) = std::env::var("CDUO_STATE_DIR") {
        return PathBuf::from(dir);
    }

    if let Some(proj_dirs) =
        ProjectDirs::from(PROJECT_QUALIFIER, PROJECT_ORGANIZATION, PROJECT_APPLICATION)
    {
        return proj_dirs
            .state_dir()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| proj_dirs.data_dir().to_path_buf());
    }

    PathBuf::from("/tmp/cduo-state")
}
