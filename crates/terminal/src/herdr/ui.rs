use std::path::Path;
use std::path::PathBuf;

use crate::Error;

pub const UI_CONFIG: &str = "# written by rivethub on every start — edits here are overwritten
[ui]
sidebar_start_collapsed = true
sidebar_collapsed_mode = \"hidden\"
hide_tab_bar_when_single_tab = true
prompt_new_tab_name = false
confirm_close = false
pane_outer_borders = false
pane_scrollbars = false
pane_gaps = false
window_title = \"{terminal_title}\"
mobile_width_threshold = 0
";

#[must_use]
pub fn ui_config_path(config_home: &Path) -> PathBuf {
    config_home.join("herdr").join("config.toml")
}

pub async fn write_ui_config(config_home: &Path) -> Result<PathBuf, Error> {
    let dir = config_home.join("herdr");
    tokio::fs::create_dir_all(&dir).await?;
    let path = ui_config_path(config_home);
    let staged = dir.join(format!(".config.toml.{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::write(&staged, UI_CONFIG).await?;
    if let Err(error) = tokio::fs::rename(&staged, &path).await {
        if let Err(remove_error) = tokio::fs::remove_file(&staged).await {
            tracing::warn!(%remove_error, "herdr ui config temp file remains");
        }
        return Err(Error::Io(error));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::UI_CONFIG;
    use super::ui_config_path;
    use super::write_ui_config;

    #[tokio::test]
    async fn the_ui_config_is_created_and_rewritten() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let config_home = tmp.path().join("ns");
        let path = write_ui_config(&config_home).await?;
        assert_eq!(path, ui_config_path(&config_home));
        assert_eq!(std::fs::read_to_string(&path)?, UI_CONFIG);
        std::fs::write(&path, "[ui]\nconfirm_close = true\n")?;
        write_ui_config(&config_home).await?;
        assert_eq!(std::fs::read_to_string(&path)?, UI_CONFIG);
        let entries = std::fs::read_dir(config_home.join("herdr"))?.count();
        assert_eq!(entries, 1);
        Ok(())
    }
}
