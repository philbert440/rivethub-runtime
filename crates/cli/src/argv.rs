pub(crate) struct Scan<'a> {
    args: &'a [String],
    at: usize,
}

pub(crate) enum Taken {
    Absent,
    Missing,
    Value(String),
}

impl<'a> Scan<'a> {
    pub(crate) fn new(args: &'a [String]) -> Self {
        Self { args, at: 0 }
    }

    pub(crate) fn current(&self) -> Option<&str> {
        self.args.get(self.at).map(String::as_str)
    }

    pub(crate) fn bump(&mut self) {
        self.at += 1;
    }

    pub(crate) fn flag(&mut self, name: &str) -> bool {
        if self.current() == Some(name) {
            self.bump();
            true
        } else {
            false
        }
    }

    pub(crate) fn option(&mut self, long: &str, short: Option<&str>) -> Taken {
        let Some(current) = self.current().map(str::to_owned) else {
            return Taken::Absent;
        };
        if let Some(rest) = strip_equals(&current, long) {
            self.bump();
            return taken(rest);
        }
        if current == long {
            return self.take_next();
        }
        let Some(short) = short else {
            return Taken::Absent;
        };
        if let Some(rest) = strip_equals(&current, short) {
            self.bump();
            return taken(rest);
        }
        if current == short {
            return self.take_next();
        }
        if let Some(rest) = current.strip_prefix(short)
            && !rest.is_empty()
            && !current.starts_with("--")
        {
            self.bump();
            return Taken::Value(rest.to_owned());
        }
        Taken::Absent
    }

    fn take_next(&mut self) -> Taken {
        self.bump();
        let Some(next) = self.current().map(str::to_owned) else {
            return Taken::Missing;
        };
        if next.starts_with('-') && next.parse::<i64>().is_err() {
            return Taken::Missing;
        }
        self.bump();
        Taken::Value(next)
    }
}

fn strip_equals<'a>(token: &'a str, flag: &str) -> Option<&'a str> {
    token.strip_prefix(flag)?.strip_prefix('=')
}

fn taken(rest: &str) -> Taken {
    if rest.is_empty() {
        Taken::Missing
    } else {
        Taken::Value(rest.to_owned())
    }
}
