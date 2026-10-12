use rustc_lint::{LateContext, LateLintPass};

use crate::util;

dylint_linting::impl_late_lint!(
    pub(crate) UNTYPED,
    Warn,
    "raw serde value outside its sanctioned holders (a typed struct, wire::JsonObject, or config::FreeForm) → docs/PATTERNS.md#untyped",
    Untyped::default()
);

#[derive(Default)]
pub(crate) struct Untyped;

const BANNED: [(&str, &str, bool); 6] = [
    ("serde_json", "Value", true),
    ("serde_json", "Map", true),
    ("serde_yaml", "Value", false),
    ("serde_yaml", "Mapping", false),
    ("serde_yaml", "Number", false),
    ("serde_yaml", "Sequence", false),
];

impl<'tcx> LateLintPass<'tcx> for Untyped {
    fn check_ty(
        &mut self,
        cx: &LateContext<'tcx>,
        ty: &'tcx rustc_hir::Ty<'tcx, rustc_hir::AmbigArg>,
    ) {
        let Some(segments) = util::segments(ty) else {
            return;
        };
        if segments.len() < 2 {
            return;
        }
        let head = segments[segments.len() - 2].ident.name.as_str();
        let last = segments[segments.len() - 1].ident.name.as_str();
        let banned = if last == "IgnoredAny" {
            Some(false)
        } else {
            BANNED
                .iter()
                .find(|(krate, name, _)| *krate == head && *name == last)
                .map(|(_, _, is_json)| *is_json)
        };
        let Some(is_json) = banned else {
            return;
        };
        if util::in_test_code(cx, ty.hir_id, ty.span) {
            return;
        }
        let crate_name = util::crate_name(cx);
        if crate_name == "testkit" {
            return;
        }
        if crate_name.starts_with("wire")
            && is_json
            && util::file_path(cx, ty.span).is_some_and(|path| path.ends_with("src/json.rs"))
        {
            return;
        }
        if crate_name == "config"
            && !is_json
            && util::file_path(cx, ty.span).is_some_and(|path| {
                ["src/load.rs", "src/resolve.rs", "src/free_form.rs"]
                    .iter()
                    .any(|tail| path.ends_with(tail))
            })
        {
            return;
        }
        util::lint(
            cx,
            UNTYPED,
            ty.hir_id,
            ty.span,
            format!(
                "{head}::{last} outside its sanctioned holder (a typed struct, wire::JsonObject, or config::FreeForm) → docs/PATTERNS.md#untyped"
            ),
        );
    }
}
