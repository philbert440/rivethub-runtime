use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub(super) struct TurnLedger {
    seen: BTreeSet<String>,
    latest: Option<(String, u64)>,
    closed: BTreeSet<String>,
    last_user: Option<u64>,
    last_completion: Option<u64>,
}

impl TurnLedger {
    pub(super) fn prompt(&mut self, id: &str, line: u64) {
        if self.seen.insert(id.to_owned()) {
            self.latest = Some((id.to_owned(), line));
        }
    }

    pub(super) fn user(&mut self, line: u64) {
        self.last_user = Some(line);
    }

    pub(super) fn completed(&mut self, id: &str, line: u64) {
        self.prompt(id, line);
        self.closed.insert(id.to_owned());
        self.last_completion = Some(line);
    }

    pub(super) fn settled(&self, last: u64) -> u64 {
        let open_prompt = self
            .latest
            .as_ref()
            .filter(|(id, _)| !self.closed.contains(id))
            .map(|(_, line)| *line);
        let open_user = self
            .last_user
            .filter(|user| self.last_completion.is_none_or(|done| done < *user));
        open_prompt
            .into_iter()
            .chain(open_user)
            .min()
            .map_or(last, |start| start.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::TurnLedger;

    #[test]
    fn a_completed_turn_settles_the_whole_file() {
        let mut ledger = TurnLedger::default();
        ledger.user(1);
        ledger.prompt("a", 2);
        ledger.completed("a", 5);
        assert_eq!(ledger.settled(7), 7);
    }

    #[test]
    fn an_open_turn_holds_back_from_its_user_message() {
        let mut ledger = TurnLedger::default();
        ledger.user(1);
        ledger.prompt("a", 2);
        ledger.completed("a", 5);
        ledger.user(7);
        assert_eq!(ledger.settled(8), 6);
        ledger.prompt("b", 8);
        assert_eq!(ledger.settled(9), 6);
        ledger.completed("b", 10);
        assert_eq!(ledger.settled(10), 10);
    }

    #[test]
    fn a_user_message_written_after_its_reply_started_holds_from_the_reply() {
        let mut ledger = TurnLedger::default();
        ledger.prompt("a", 3);
        ledger.user(5);
        assert_eq!(ledger.settled(6), 2);
    }

    #[test]
    fn a_later_turn_closes_an_earlier_one_that_never_completed() {
        let mut ledger = TurnLedger::default();
        ledger.user(1);
        ledger.prompt("a", 2);
        ledger.prompt("b", 6);
        ledger.completed("b", 9);
        assert_eq!(ledger.settled(9), 9);
    }
}
