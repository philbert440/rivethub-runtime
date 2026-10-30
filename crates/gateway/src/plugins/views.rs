use wire::plugin::view::{View, ViewBlock};

const SCENE_LIMIT: usize = 5000;

pub(crate) fn check(view: &View) -> Result<(), String> {
    let items = canvas_items(&view.blocks);
    if items > SCENE_LIMIT {
        Err(format!(
            "blocks: {items} canvas items, more than {SCENE_LIMIT}"
        ))
    } else {
        Ok(())
    }
}

fn canvas_items(blocks: &[ViewBlock]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            ViewBlock::Canvas { scene, .. } => scene.items.len(),
            ViewBlock::Section { blocks, .. } => canvas_items(blocks),
            ViewBlock::Split {
                primary, secondary, ..
            } => canvas_items(primary) + canvas_items(secondary),
            ViewBlock::Tabs { tabs, .. } => tabs.iter().map(|tab| canvas_items(&tab.blocks)).sum(),
            ViewBlock::Text { .. }
            | ViewBlock::Markdown { .. }
            | ViewBlock::Code { .. }
            | ViewBlock::Image { .. }
            | ViewBlock::Properties { .. }
            | ViewBlock::Progress { .. }
            | ViewBlock::Notice { .. }
            | ViewBlock::List { .. }
            | ViewBlock::Table { .. }
            | ViewBlock::Breadcrumbs { .. }
            | ViewBlock::Actions { .. }
            | ViewBlock::Form { .. }
            | ViewBlock::WebPanel { .. } => 0,
        })
        .sum()
}
