use kernel::transcript::StoreSession;

use super::text::clip;

const TITLE_CHARS: usize = 120;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HarnessTitles {
    manual: Option<String>,
    automatic: Option<String>,
}

impl HarnessTitles {
    pub(crate) fn manual(&mut self, title: &str) {
        if let Some(title) = cleaned(title) {
            self.manual = Some(title);
        }
    }

    pub(crate) fn automatic(&mut self, title: &str) {
        if let Some(title) = cleaned(title) {
            self.automatic = Some(title);
        }
    }

    pub(crate) fn apply(self, row: &mut StoreSession) {
        if let Some(title) = self.manual.or(self.automatic) {
            row.title = title;
        }
    }
}

fn cleaned(title: &str) -> Option<String> {
    let clipped = clip(title, TITLE_CHARS);
    (!clipped.is_empty()).then_some(clipped)
}
