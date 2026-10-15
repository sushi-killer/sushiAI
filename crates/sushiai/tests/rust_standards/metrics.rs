use super::model::{FunctionMetric, MacroMetric};
use super::syntax::{attributes, joined, test_only};
use std::collections::BTreeSet;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

pub struct Metrics {
    path: String,
    scope: Vec<String>,
    test: bool,
    pub test_lines: BTreeSet<usize>,
    pub functions: Vec<FunctionMetric>,
    pub macros: Vec<MacroMetric>,
}

impl Metrics {
    pub fn new(path: &str, module: &[String], test: bool) -> Self {
        Self {
            path: path.into(),
            scope: module.to_vec(),
            test,
            test_lines: BTreeSet::new(),
            functions: Vec::new(),
            macros: Vec::new(),
        }
    }

    fn mark_test(&mut self, span: proc_macro2::Span) {
        if self.test {
            self.test_lines.extend(span.start().line..=span.end().line);
        }
    }

    fn function(&mut self, sig: &syn::Signature, end: proc_macro2::Span) {
        let line = sig.fn_token.span.start().line;
        let mut symbol = self.scope.clone();
        symbol.push(sig.ident.to_string());
        self.functions.push(FunctionMetric {
            path: self.path.clone(),
            symbol: joined(&symbol),
            line,
            lines: end.end().line.saturating_sub(line) + 1,
            arguments: sig.inputs.len(),
            test: self.test,
        });
    }
}

impl<'ast> Visit<'ast> for Metrics {
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        let name = attr
            .path()
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");
        if ![
            "cfg",
            "allow",
            "expect",
            "deny",
            "forbid",
            "doc",
            "path",
            "test",
            "ignore",
            "should_panic",
            "repr",
            "inline",
            "cold",
            "must_use",
            "no_mangle",
            "export_name",
            "link_section",
            "deprecated",
            "non_exhaustive",
            "track_caller",
            "automatically_derived",
        ]
        .contains(&name.as_str())
        {
            let mut symbol = self.scope.clone();
            symbol.push(format!("@{name}"));
            self.macros.push(MacroMetric {
                kind: "attribute",
                path: self.path.clone(),
                symbol: joined(&symbol),
                line: attr.span().start().line,
                lines: attr.span().end().line - attr.span().start().line + 1,
            });
        }
    }

    fn visit_item(&mut self, item: &'ast syn::Item) {
        let previous = self.test;
        self.test |= test_only(attributes(item));
        self.mark_test(item.span());
        visit::visit_item(self, item);
        self.test = previous;
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        self.scope.push(item.ident.to_string());
        visit::visit_item_mod(self, item);
        self.scope.pop();
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let name = if let syn::Type::Path(ty) = &*item.self_ty {
            ty.path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_else(|| "impl".into())
        } else {
            "impl".into()
        };
        let name = item.trait_.as_ref().map_or(name.clone(), |(_, path, _)| {
            format!(
                "{name} as {}",
                path.segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::")
            )
        });
        self.scope.push(name);
        visit::visit_item_impl(self, item);
        self.scope.pop();
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        self.scope.push(item.ident.to_string());
        visit::visit_item_trait(self, item);
        self.scope.pop();
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        self.function(&item.sig, item.block.span());
        self.scope.push(item.sig.ident.to_string());
        visit::visit_item_fn(self, item);
        self.scope.pop();
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        let previous = self.test;
        self.test |= test_only(&item.attrs);
        self.mark_test(item.span());
        self.function(&item.sig, item.block.span());
        self.scope.push(item.sig.ident.to_string());
        visit::visit_impl_item_fn(self, item);
        self.scope.pop();
        self.test = previous;
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        let previous = self.test;
        self.test |= test_only(&item.attrs);
        self.mark_test(item.span());
        self.function(
            &item.sig,
            item.default.as_ref().map_or(item.sig.span(), Spanned::span),
        );
        self.scope.push(item.sig.ident.to_string());
        visit::visit_trait_item_fn(self, item);
        self.scope.pop();
        self.test = previous;
    }

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if item.mac.path.is_ident("include") {
            return;
        }
        let name = item
            .ident
            .as_ref()
            .map(|i| i.to_string())
            .unwrap_or_else(|| {
                item.mac
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::")
            });
        let mut symbol = self.scope.clone();
        symbol.push(format!("{name}!"));
        self.macros.push(MacroMetric {
            kind: "item",
            path: self.path.clone(),
            symbol: joined(&symbol),
            line: item.span().start().line,
            lines: item.span().end().line - item.span().start().line + 1,
        });
    }
}
