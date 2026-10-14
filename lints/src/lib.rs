#![feature(rustc_private)]

extern crate rustc_ast;
extern crate rustc_attr_ir;
extern crate rustc_errors;
extern crate rustc_hir;
extern crate rustc_lint;
extern crate rustc_middle;
extern crate rustc_session;
extern crate rustc_span;

mod util;

mod counttype;
mod customserde;
mod dispatch;
mod dupnames;
mod duptype;
mod dynvalue;
mod fallback;
mod fallback_policy;
mod fallback_waiver;
mod jsmodel;
mod manualderive;
mod naming;
mod nullable;
mod opaque;
mod redundantrename;
mod runtime;
mod sleep;
mod stringly;
mod strset;
mod tagfield;
mod typemacro;
mod untyped;
mod wirepub;

use rustc_lint::LintStore;
use rustc_session::Session;

dylint_linting::dylint_library!();

#[unsafe(no_mangle)]
pub fn register_lints(sess: &Session, lint_store: &mut LintStore) {
    dylint_linting::init_config(sess);

    lint_store.register_lints(&[
        counttype::COUNTTYPE,
        customserde::CUSTOMSERDE,
        dispatch::DISPATCH,
        dupnames::DUPNAMES,
        fallback::FALLBACK,
        fallback_waiver::FALLBACK_WAIVER,
        duptype::DUPTYPE,
        dynvalue::DYNVALUE,
        jsmodel::JSMODEL,
        manualderive::MANUALDERIVE,
        naming::NAMING,
        nullable::NULLABLE,
        opaque::OPAQUE,
        redundantrename::REDUNDANT_RENAME,
        runtime::RUNTIME,
        sleep::SLEEP,
        stringly::STRINGLY,
        strset::STRSET,
        tagfield::TAGFIELD,
        typemacro::TYPEMACRO,
        untyped::UNTYPED,
        wirepub::WIREPUB,
    ]);

    lint_store.register_pre_expansion_lint_pass(Box::new(|| {
        Box::new(runtime::RuntimeEarly::default()) as Box<dyn rustc_lint::EarlyLintPass>
    }));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(counttype::Counttype::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(customserde::Customserde::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(dispatch::Dispatch::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(dupnames::Dupnames::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(duptype::Duptype::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(dynvalue::Dynvalue::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(jsmodel::Jsmodel::default())));
    lint_store.register_late_lint_pass(Box::new(|_| {
        Box::new(manualderive::Manualderive::default())
    }));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(naming::Naming::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(nullable::Nullable::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(opaque::Opaque::default())));
    lint_store.register_late_lint_pass(Box::new(|_| {
        Box::new(redundantrename::RedundantRename::default())
    }));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(runtime::RuntimeLate::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(fallback::Fallback)));
    lint_store.register_late_lint_pass(Box::new(|_| {
        Box::new(fallback_waiver::FallbackWaiver::default())
    }));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(sleep::Sleep::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(stringly::Stringly::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(strset::Strset::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(tagfield::Tagfield::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(typemacro::Typemacro::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(untyped::Untyped::default())));
    lint_store.register_late_lint_pass(Box::new(|_| Box::new(wirepub::Wirepub::default())));
}
