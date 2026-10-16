pub(super) struct Line<'a> {
    pub text: &'a str,
    pub fenced: bool,
}

pub(super) fn fence_aware(text: &str) -> impl Iterator<Item = Line<'_>> {
    let mut open = false;
    text.split('\n').map(move |text| {
        let marker = text.trim_start().starts_with("```");
        let fenced = open || marker;
        if marker {
            open = !open;
        }
        Line { text, fenced }
    })
}
