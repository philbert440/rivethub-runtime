use url::Url;

const REDIRECT_CAP: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectChoice {
    Follow,
    Stop,
    Cap,
}

pub(crate) fn redirect_choice(previous: &[Url], next: &Url) -> RedirectChoice {
    if previous.len() > REDIRECT_CAP {
        return RedirectChoice::Cap;
    }
    let Some(prior) = previous.last() else {
        return RedirectChoice::Cap;
    };
    if allowed(prior, next) {
        RedirectChoice::Follow
    } else {
        RedirectChoice::Stop
    }
}

pub(crate) fn allowed(previous: &Url, next: &Url) -> bool {
    if userinfo(previous) || userinfo(next) {
        return false;
    }
    if previous.host_str().is_none() || previous.host_str() != next.host_str() {
        return false;
    }
    let same_port = previous.port_or_known_default() == next.port_or_known_default();
    let same_scheme = previous.scheme() == next.scheme() && (http(previous) || https(previous));
    let upgrade = http(previous) && https(next);
    if same_port && (same_scheme || upgrade) {
        return true;
    }
    upgrade
        && previous.port_or_known_default() == Some(80)
        && next.port_or_known_default() == Some(443)
}

fn userinfo(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

fn http(url: &Url) -> bool {
    url.scheme() == "http"
}

fn https(url: &Url) -> bool {
    url.scheme() == "https"
}

pub(crate) fn redirect_action(
    attempt: reqwest::redirect::Attempt<'_>,
) -> reqwest::redirect::Action {
    match redirect_choice(attempt.previous(), attempt.url()) {
        RedirectChoice::Follow => attempt.follow(),
        RedirectChoice::Stop => attempt.error("redirect refused"),
        RedirectChoice::Cap => attempt.error("redirect cap"),
    }
}

#[cfg(test)]
mod tests {
    use super::RedirectChoice;
    use super::allowed;
    use super::redirect_choice;
    use url::Url;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn redirects_stay_on_the_same_host_and_do_not_downgrade() {
        let http = url("http://files.example:8080/a");
        let https_same = url("https://files.example:8080/b");
        let plain = url("http://files.example/a");
        let tls = url("https://files.example/b");
        assert!(allowed(&http, &https_same));
        assert!(allowed(&plain, &tls));
        assert!(allowed(&tls, &url("https://files.example/c")));
        assert!(!allowed(&tls, &plain));
        assert!(!allowed(&http, &url("https://files.example:8443/b")));
        assert!(!allowed(&plain, &url("http://other.example/b")));
        assert!(!allowed(
            &url("http://user:pass@files.example/a"),
            &url("http://files.example/b")
        ));
        assert!(!allowed(&plain, &url("http://user@files.example/b")));
    }

    #[test]
    fn the_sixth_redirect_hits_the_cap() {
        let next = url("http://files.example/next");
        let mut previous = vec![url("http://files.example/start")];
        assert_eq!(redirect_choice(&previous, &next), RedirectChoice::Follow);
        while previous.len() <= 5 {
            previous.push(url("http://files.example/hop"));
        }
        assert_eq!(redirect_choice(&previous, &next), RedirectChoice::Cap);
        assert_eq!(redirect_choice(&[], &next), RedirectChoice::Cap);
        assert_eq!(
            redirect_choice(
                &[url("http://files.example/a")],
                &url("http://other.example/b")
            ),
            RedirectChoice::Stop
        );
    }
}
