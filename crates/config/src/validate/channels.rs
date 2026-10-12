use std::collections::BTreeMap;

use crate::model::ChannelEntry;
use crate::report::ValidationResult;

pub(super) fn validate(out: &mut ValidationResult, channels: &BTreeMap<String, ChannelEntry>) {
    for (id, channel) in channels {
        if id != "agent" {
            out.warning(
                format!("channels.{id}"),
                format!("Unknown channel type \"{id}\" — make sure a registrar handles it"),
            );
        }
        bot_token(out, id, channel);
    }
}

fn bot_token(out: &mut ValidationResult, id: &str, channel: &ChannelEntry) {
    let Some(token) = &channel.bot_token else {
        return;
    };
    if token.chars().count() > 20 && !token.contains("${") {
        out.warning(
            format!("channels.{id}.bot_token"),
            format!("Channel \"{id}\" appears to have a hardcoded bot token — use environment variables instead"),
        );
    }
}
