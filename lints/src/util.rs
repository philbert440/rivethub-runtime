use rustc_attr_ir::{Attribute, AttributeKind, CfgEntry};
use rustc_hir::{
    GenericArg, GenericArgs, HirId, QPath, Ty, TyKind,
    def::{DefKind, Res},
};
use rustc_lint::LateContext;
use rustc_session::Session;
use rustc_span::{Span, Symbol, def_id::DefId, def_id::LOCAL_CRATE, sym};
use std::ffi::OsStr;
use std::path::Path;

pub fn crate_name(cx: &LateContext<'_>) -> String {
    cx.tcx.crate_name(LOCAL_CRATE).to_string()
}

pub fn sess_crate_name(sess: &Session) -> String {
    sess.opts.crate_name.clone().unwrap_or_default()
}

pub fn symbol(name: &str) -> Symbol {
    Symbol::intern(name)
}

fn sess_file_path(sess: &Session, span: Span) -> Option<String> {
    match sess.source_map().span_to_filename(span) {
        rustc_span::FileName::Real(path) => Some(path.local_path()?.display().to_string()),
        _ => None,
    }
}

pub fn file_path(cx: &LateContext<'_>, span: Span) -> Option<String> {
    sess_file_path(cx.tcx.sess, span)
}

pub fn package(cx: &LateContext<'_>, span: Span) -> Option<String> {
    let path = file_path(cx, span)?;
    let mut components = Path::new(&path).components();
    while let Some(component) = components.next() {
        if component.as_os_str() != "crates" {
            continue;
        }
        return components
            .next()
            .map(|next| next.as_os_str().to_string_lossy().to_string());
    }
    None
}

pub fn package_or_crate(cx: &LateContext<'_>, span: Span) -> String {
    package(cx, span).unwrap_or_else(|| crate_name(cx))
}

fn is_composition_root(crate_name: &str, path: &str) -> bool {
    let file = Path::new(path).file_name();
    match crate_name {
        "rivethub" | "rh" => file == Some(OsStr::new("main.rs")),
        "rivetos" => {
            file == Some(OsStr::new("main.rs")) || file == Some(OsStr::new("rivetos.rs"))
        }
        _ => false,
    }
}

pub fn in_test_file(cx: &LateContext<'_>, span: Span) -> bool {
    file_path(cx, span).is_some_and(|path| path.contains("/tests/"))
}

pub fn in_test_file_sess(sess: &Session, span: Span) -> bool {
    sess_file_path(sess, span).is_some_and(|path| path.contains("/tests/"))
}

pub fn in_rivethub_main(cx: &LateContext<'_>, span: Span) -> bool {
    file_path(cx, span).is_some_and(|path| is_composition_root(&crate_name(cx), &path))
}

pub fn in_rivethub_main_sess(sess: &Session, span: Span) -> bool {
    sess_file_path(sess, span)
        .is_some_and(|path| is_composition_root(&sess_crate_name(sess), &path))
}

pub fn in_cfg_test(cx: &LateContext<'_>, hir_id: HirId) -> bool {
    cx.tcx
        .hir_parent_id_iter(hir_id)
        .any(|id| cx.tcx.hir_attrs(id).iter().any(is_cfg_test))
}

pub fn in_test_code(cx: &LateContext<'_>, hir_id: HirId, span: Span) -> bool {
    in_test_file(cx, span) || in_cfg_test(cx, hir_id)
}

fn is_cfg_test(attr: &Attribute) -> bool {
    if let Attribute::Parsed(AttributeKind::CfgTrace(cfgs)) = attr {
        return cfgs.iter().any(|(entry, _)| cfg_entry_is_test(entry));
    }
    attr.has_name(sym::cfg)
        && attr
            .meta_item_list()
            .is_some_and(|list| list.iter().any(|item| item.ident().map(|i| i.name) == Some(sym::test)))
}

fn cfg_entry_is_test(entry: &CfgEntry) -> bool {
    match entry {
        CfgEntry::NameValue {
            name,
            value: None,
            span: _,
        } => *name == sym::test,
        CfgEntry::All(subs, _) | CfgEntry::Any(subs, _) => subs.iter().any(cfg_entry_is_test),
        _ => false,
    }
}

pub fn has_pub_keyword(cx: &LateContext<'_>, vis_span: Span) -> bool {
    cx.tcx
        .sess
        .source_map()
        .span_to_snippet(vis_span)
        .is_ok_and(|snippet| !snippet.trim().is_empty())
}

pub fn identifier_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = name.chars().collect();
    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '_' {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        let split_before = ch.is_ascii_uppercase()
            && !current.is_empty()
            && (current
                .chars()
                .next_back()
                .is_some_and(|last| last.is_ascii_lowercase() || last.is_ascii_digit())
                || chars
                    .get(index + 1)
                    .is_some_and(|next| next.is_ascii_lowercase()));
        if split_before {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

pub fn final_name_word(name: &str) -> String {
    identifier_words(name)
        .last()
        .cloned()
        .unwrap_or_else(|| name.to_owned())
}

pub fn banned_name(name: &str) -> bool {
    identifier_words(name)
        .iter()
        .any(|word| matches!(word.as_str().to_lowercase().as_str(), "den" | "room" | "rooms"))
}

pub fn snake(name: Symbol) -> String {
    let mut out = String::new();
    for ch in name.as_str().chars() {
        if ch.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn segments<'hir, U>(ty: &'hir Ty<'hir, U>) -> Option<&'hir [rustc_hir::PathSegment<'hir>]> {
    match &ty.kind {
        TyKind::Path(QPath::Resolved(_, path)) => Some(path.segments),
        _ => None,
    }
}

pub fn type_name<'hir, U>(ty: &'hir Ty<'hir, U>) -> Option<Symbol> {
    segments(ty)
        .and_then(|segs| segs.last())
        .map(|seg| seg.ident.name)
}

fn type_arg<'a>(
    args: Option<&'a GenericArgs<'a>>,
) -> Option<&'a Ty<'a, rustc_hir::AmbigArg>> {
    let args = args?;
    args.args.iter().find_map(|arg| match arg {
        GenericArg::Type(ty) => Some(*ty),
        _ => None,
    })
}

pub fn wrapper_inner<'hir, U>(
    ty: &'hir Ty<'hir, U>,
    wrappers: &[Symbol],
) -> Option<&'hir Ty<'hir, rustc_hir::AmbigArg>> {
    let segs = segments(ty)?;
    if segs.len() != 1 || !wrappers.contains(&segs[0].ident.name) {
        return None;
    }
    type_arg(segs[0].args)
}

pub fn string_like<'hir, U>(ty: &'hir Ty<'hir, U>) -> bool {
    if type_name(ty) == Some(sym::String) {
        return true;
    }
    wrapper_inner(ty, &[sym::Option, sym::Vec]).is_some_and(string_like)
}

pub fn option_inner<'hir, U>(ty: &'hir Ty<'hir, U>) -> Option<&'hir Ty<'hir, rustc_hir::AmbigArg>> {
    wrapper_inner(ty, &[sym::Option])
}

pub fn option_option<'hir, U>(ty: &'hir Ty<'hir, U>) -> bool {
    option_inner(ty).is_some_and(|inner| option_inner(inner).is_some())
}

pub fn resolved_enum(cx: &LateContext<'_>, ty: &Ty<'_>) -> Option<DefId> {
    match &ty.kind {
        TyKind::Path(QPath::Resolved(_, path)) => match path.res {
            Res::Def(DefKind::Enum, did) => Some(did),
            _ => None,
        },
        _ => {
            let _ = cx;
            None
        }
    }
}

pub fn is_str_type<'hir, U>(ty: &'hir Ty<'hir, U>) -> bool {
    match &ty.kind {
        TyKind::Ref(_, inner, _) => is_str_type(inner),
        _ => type_name(ty).is_some_and(|name| name.as_str() == "str"),
    }
}

pub fn str_table<'hir, U>(ty: &'hir Ty<'hir, U>) -> bool {
    match &ty.kind {
        TyKind::Ref(_, inner, _) => str_table(inner),
        TyKind::Slice(elem) | TyKind::Array(elem, _) => is_str_type(elem),
        TyKind::Tup(parts) => parts.first().is_some_and(is_str_type),
        _ => false,
    }
}

pub fn serde_attr_pairs(attrs: &[Attribute]) -> Vec<(Symbol, Option<Symbol>)> {
    let serde = symbol("serde");
    attrs
        .iter()
        .filter(|attr| attr.has_name(serde))
        .filter_map(|attr| attr.meta_item_list())
        .flatten()
        .filter_map(|inner| {
            let meta = inner.meta_item()?;
            let key = meta.path.segments.last()?.ident.name;
            Some((key, meta.value_str()))
        })
        .collect()
}

pub fn lint(
    cx: &LateContext<'_>,
    l: &'static rustc_lint::Lint,
    hir_id: HirId,
    span: Span,
    msg: String,
) {
    if span.in_external_macro(cx.tcx.sess.source_map()) {
        return;
    }
    cx.tcx.emit_node_span_lint(
        l,
        hir_id,
        span,
        rustc_errors::DiagDecorator(|diag| {
            diag.primary_message(msg);
        }),
    );
}
