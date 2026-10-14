use std::collections::BTreeSet;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned as _;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, Meta, Stmt, Token};

pub fn violations(source: &str) -> Result<Vec<usize>, syn::Error> {
    let file = syn::parse_file(source)?;
    let mut visitor = Policy::default();
    visitor.visit_file(&file);
    Ok(visitor.invalid)
}

#[derive(Default)]
struct Policy {
    statements: BTreeSet<(usize, usize)>,
    invalid: Vec<usize>,
}

impl<'ast> Visit<'ast> for Policy {
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        let attrs = match stmt {
            Stmt::Local(local) => &local.attrs[..],
            Stmt::Macro(mac) => &mac.attrs[..],
            Stmt::Expr(expr, _) => expression_attrs(expr),
            Stmt::Item(_) => &[],
        };
        for attr in attrs {
            let start: proc_macro2::LineColumn = attr.span().start();
            self.statements.insert((start.line, start.column));
        }
        visit::visit_stmt(self, stmt);
    }

    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        let start: proc_macro2::LineColumn = attr.span().start();
        let statement = matches!(attr.style, syn::AttrStyle::Outer)
            && self.statements.contains(&(start.line, start.column));
        if invalid_meta(&attr.meta, statement) {
            self.invalid.push(start.line);
        }
    }
}

fn expression_attrs(expr: &Expr) -> &[Attribute] {
    match expr {
        Expr::MethodCall(expr) => &expr.attrs,
        Expr::Call(expr) => &expr.attrs,
        Expr::Match(expr) => &expr.attrs,
        Expr::If(expr) => &expr.attrs,
        Expr::Block(expr) => &expr.attrs,
        Expr::Return(expr) => &expr.attrs,
        Expr::Await(expr) => &expr.attrs,
        Expr::Try(expr) => &expr.attrs,
        Expr::Path(expr) => &expr.attrs,
        Expr::Lit(expr) => &expr.attrs,
        Expr::Assign(expr) => &expr.attrs,
        Expr::Break(expr) => &expr.attrs,
        Expr::Continue(expr) => &expr.attrs,
        Expr::Loop(expr) => &expr.attrs,
        Expr::While(expr) => &expr.attrs,
        Expr::ForLoop(expr) => &expr.attrs,
        Expr::Macro(expr) => &expr.attrs,
        _ => &[],
    }
}

fn invalid_meta(meta: &Meta, statement: bool) -> bool {
    let Meta::List(list) = meta else { return false };
    let Ok(parts) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
        return false;
    };
    if list.path.is_ident("cfg_attr") {
        return parts
            .iter()
            .skip(1)
            .any(|part| invalid_meta(part, statement));
    }
    if !list.path.is_ident("expect") && !list.path.is_ident("allow") {
        return false;
    }
    let fallback = parts
        .iter()
        .any(|part| part.path().is_ident("fallback") || part.path().is_ident("fallback_waiver"));
    if !fallback {
        return false;
    }
    let reason = parts.iter().find_map(|part| {
        if let Meta::NameValue(pair) = part
            && pair.path.is_ident("reason")
            && let Expr::Lit(value) = &pair.value
            && let syn::Lit::Str(text) = &value.lit
        {
            Some(text.value())
        } else {
            None
        }
    });
    !statement
        || !list.path.is_ident("expect")
        || !reason.as_deref().is_some_and(cites_rule)
        || parts
            .iter()
            .any(|part| part.path().is_ident("fallback_waiver"))
}

pub(crate) fn cites_rule(reason: &str) -> bool {
    let words: Vec<_> = reason
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|word| !word.is_empty())
        .collect();
    for (index, word) in words.iter().enumerate() {
        let mut tail = words.iter().skip(index + 1).copied();
        let next = tail.next();
        let id = if next == Some("md") {
            tail.next()
        } else {
            next
        };
        if *word == "CONTRACT"
            && id.is_some_and(|id| {
                id.strip_prefix('B')
                    .and_then(|number| number.parse::<u8>().ok())
                    .is_some_and(|row| (1..=16).contains(&row))
            })
        {
            return true;
        }
        if *word == "PATTERNS"
            && id.is_some_and(|id| {
                include_str!("../../docs/PATTERNS.md")
                    .lines()
                    .filter_map(|line| line.strip_prefix("## "))
                    .any(|name| name.eq_ignore_ascii_case(id))
            })
        {
            return true;
        }
    }
    false
}
