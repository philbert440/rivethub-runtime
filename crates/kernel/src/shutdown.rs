use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Default)]
pub struct Activities {
    names: Arc<Mutex<Vec<String>>>,
}

#[derive(Debug)]
#[must_use]
pub struct ActivityGuard {
    names: Arc<Mutex<Vec<String>>>,
    name: String,
}

impl Activities {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enter(&self, name: impl Into<String>) -> ActivityGuard {
        let name = name.into();
        self.names.lock().push(name.clone());
        ActivityGuard {
            names: Arc::clone(&self.names),
            name,
        }
    }

    #[must_use]
    pub fn list(&self) -> String {
        let guard = self.names.lock();
        if guard.is_empty() {
            "(none)".to_owned()
        } else {
            guard.join(", ")
        }
    }
}

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        let mut guard = self.names.lock();
        if let Some(index) = guard.iter().rposition(|item| item == &self.name) {
            guard.swap_remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Activities;

    #[test]
    fn list_names_entered_work_and_drop_removes_it() {
        let activities = Activities::new();
        assert_eq!(activities.list(), "(none)");
        let first = activities.enter("provider chat");
        let second = activities.enter("job embed");
        assert_eq!(activities.list(), "provider chat, job embed");
        drop(first);
        assert_eq!(activities.list(), "job embed");
        drop(second);
        assert_eq!(activities.list(), "(none)");
    }
}
