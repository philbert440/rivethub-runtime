use std::borrow::Cow;

use kernel::tasks::TaskError;
use kernel::tasks::TaskPresets;

use crate::directory::Directory;

pub(crate) async fn current<'a>(
    directory: &'a Directory,
    source: Option<&dyn TaskPresets>,
) -> Result<Cow<'a, Directory>, TaskError> {
    let Some(source) = source else {
        return Ok(Cow::Borrowed(directory));
    };
    let mut live = directory.clone();
    live.replace_presets(source.presets().await?);
    Ok(Cow::Owned(live))
}
